// os/src/fs/page_cache.rs

use super::vfs::InodeOp;
use crate::config::PAGE_CACHE_GLOBAL_MAX_PAGES;
use crate::config::PAGE_SIZE;
use crate::syscall::{Errno, SysResult};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;

lazy_static! {
    static ref PAGE_CACHE_REGISTRY: Mutex<BTreeMap<usize, Weak<PageCache>>> =
        Mutex::new(BTreeMap::new());
    static ref PAGE_CACHE_LRU: Mutex<VecDeque<LruEntry>> = Mutex::new(VecDeque::new());
}

static NEXT_PAGE_CACHE_ID: AtomicUsize = AtomicUsize::new(1);
static NEXT_LRU_GENERATION: AtomicUsize = AtomicUsize::new(1);
static PAGE_CACHE_PAGE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct LruEntry {
    cache_id: usize,
    page_idx: usize,
    generation: usize,
}

enum ReclaimResult {
    Removed,
    Kept,
}

/// 页缓存中的一页
pub struct Page {
    data: Vec<u8>, // PAGE_SIZE 字节
    dirty: bool,
    generation: usize,
    queued: bool,
}

impl Page {
    fn new_zeroed(generation: usize) -> Self {
        Self {
            data: alloc::vec![0u8; PAGE_SIZE],
            dirty: false,
            generation,
            queued: false,
        }
    }
}

/// 共享页缓存，挂在 inode 上。内部用 Mutex 保护 BTreeMap，
/// I/O 在锁外完成，避免持锁期间做磁盘操作。
pub struct PageCache {
    id: usize,
    pages: Mutex<BTreeMap<usize, Arc<Mutex<Page>>>>,
    file_size: Mutex<usize>,
}

impl PageCache {
    pub fn new(file_size: usize) -> Arc<Self> {
        let id = NEXT_PAGE_CACHE_ID.fetch_add(1, Ordering::Relaxed);
        let cache = Arc::new(Self {
            id,
            pages: Mutex::new(BTreeMap::new()),
            file_size: Mutex::new(file_size),
        });
        PAGE_CACHE_REGISTRY
            .lock()
            .insert(id, Arc::downgrade(&cache));
        cache
    }

    pub fn len(&self) -> usize {
        *self.file_size.lock()
    }

    fn next_generation() -> usize {
        NEXT_LRU_GENERATION.fetch_add(1, Ordering::Relaxed)
    }

    fn touch_page(&self, page_idx: usize, page: &Arc<Mutex<Page>>) {
        let generation = Self::next_generation();
        let mut page = page.lock();
        page.generation = generation;
        if page.queued {
            return;
        }
        page.queued = true;
        drop(page);
        PAGE_CACHE_LRU.lock().push_back(LruEntry {
            cache_id: self.id,
            page_idx,
            generation,
        });
    }

    fn reclaim_global() {
        while PAGE_CACHE_PAGE_COUNT.load(Ordering::Relaxed) > PAGE_CACHE_GLOBAL_MAX_PAGES {
            let Some(entry) = PAGE_CACHE_LRU.lock().pop_front() else {
                break;
            };
            let Some(cache) = PAGE_CACHE_REGISTRY
                .lock()
                .get(&entry.cache_id)
                .and_then(Weak::upgrade)
            else {
                continue;
            };
            match cache.reclaim_lru_entry(entry.page_idx, entry.generation) {
                ReclaimResult::Removed => {
                    PAGE_CACHE_PAGE_COUNT.fetch_sub(1, Ordering::Relaxed);
                }
                ReclaimResult::Kept => {}
            }
        }
    }

    fn reclaim_lru_entry(&self, page_idx: usize, generation: usize) -> ReclaimResult {
        let mut pages = self.pages.lock();
        let Some(page) = pages.get(&page_idx) else {
            return ReclaimResult::Kept;
        };
        let page = page.clone();
        {
            let mut page_guard = page.lock();
            if !page_guard.queued {
                return ReclaimResult::Kept;
            }
            page_guard.queued = false;
            if page_guard.generation != generation {
                drop(page_guard);
                drop(pages);
                self.touch_page(page_idx, &page);
                return ReclaimResult::Kept;
            }
            if page_guard.dirty || Arc::strong_count(&page) != 2 {
                return ReclaimResult::Kept;
            }
        }
        pages.remove(&page_idx);
        ReclaimResult::Removed
    }

    pub fn resize(&self, new_size: usize) {
        let mut size = self.file_size.lock();
        let mut removed_pages = 0usize;
        if new_size < *size {
            let mut pages = self.pages.lock();
            // 文件缩小时，删除所有超范围的页。
            let victims: Vec<_> = pages
                .keys()
                .copied()
                .filter(|idx| idx * PAGE_SIZE >= new_size)
                .collect();
            removed_pages = victims.len();
            for victim in victims {
                pages.remove(&victim);
            }
            if new_size % PAGE_SIZE != 0 {
                let last_page_idx = new_size / PAGE_SIZE;
                if let Some(page) = pages.get(&last_page_idx) {
                    let mut page = page.lock();
                    page.data[new_size % PAGE_SIZE..].fill(0);
                }
            }
        }
        if removed_pages != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_sub(removed_pages, Ordering::Relaxed);
        }
        *size = new_size;
    }

    /// 查 BTreeMap 获取页（不触发 I/O）
    fn lookup_page(&self, page_idx: usize) -> Option<Arc<Mutex<Page>>> {
        let page = self.pages.lock().get(&page_idx).cloned();
        if let Some(page) = page.as_ref() {
            self.touch_page(page_idx, page);
        }
        page
    }

    /// 获取页（懒加载）。I/O 成功后再插入缓存，避免失败时留下零页。
    fn get_or_load(
        &self,
        page_idx: usize,
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
    ) -> SysResult<Arc<Mutex<Page>>> {
        if let Some(page) = self.lookup_page(page_idx) {
            return Ok(page);
        }

        let file_size = *self.file_size.lock();
        let page_start = page_idx * PAGE_SIZE;
        let mut new_page = Page::new_zeroed(Self::next_generation());

        // 在 PageCache 锁外做磁盘 I/O。tmpfile 没有底层文件，保持零页。
        if page_start < file_size {
            if let Some((inode, path)) = lower {
                let page_len = (file_size - page_start).min(PAGE_SIZE);
                match inode.read_at(path, page_start, &mut new_page.data[..page_len]) {
                    Ok(_) | Err(Errno::ENOENT) => {}
                    Err(err) => return Err(err),
                }
            }
        }

        let page = Arc::new(Mutex::new(new_page));
        let mut pages = self.pages.lock();
        if let Some(existing) = pages.get(&page_idx) {
            let existing = existing.clone();
            drop(pages);
            self.touch_page(page_idx, &existing);
            return Ok(existing.clone());
        }
        pages.insert(page_idx, page.clone());
        PAGE_CACHE_PAGE_COUNT.fetch_add(1, Ordering::Relaxed);
        drop(pages);
        self.touch_page(page_idx, &page);
        Self::reclaim_global();
        Ok(page)
    }

    /// 从页缓存读取数据到 buf
    pub fn read_at(
        &self,
        offset: usize,
        buf: &mut [u8],
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
    ) -> SysResult<usize> {
        let file_size = *self.file_size.lock();
        let mut copied = 0;
        let mut pos = offset.min(file_size);
        let end = file_size.min(offset.saturating_add(buf.len()));
        while pos < end {
            let page_idx = pos / PAGE_SIZE;
            let page_off = pos % PAGE_SIZE;
            let n = (end - pos).min(PAGE_SIZE - page_off);
            let page = self.get_or_load(page_idx, lower)?;
            let p = page.lock();
            buf[copied..copied + n].copy_from_slice(&p.data[page_off..page_off + n]);
            drop(p);
            pos += n;
            copied += n;
        }
        Ok(copied)
    }

    /// 写入数据到页缓存（纯内存操作，不透写磁盘）
    pub fn write_at(
        &self,
        offset: usize,
        buf: &[u8],
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
    ) -> SysResult<usize> {
        let end = offset.checked_add(buf.len()).ok_or(Errno::EINVAL)?;
        let old_size = *self.file_size.lock();
        {
            let mut size = self.file_size.lock();
            if end > *size {
                *size = end;
            }
        }

        let mut copied = 0;
        let mut pos = offset;
        while copied < buf.len() {
            let page_idx = pos / PAGE_SIZE;
            let page_off = pos % PAGE_SIZE;
            let n = (buf.len() - copied).min(PAGE_SIZE - page_off);
            let page_start = page_idx * PAGE_SIZE;
            let old_page_end = old_size.min(page_start + PAGE_SIZE);
            let full_page_write = page_off == 0 && n == PAGE_SIZE;
            let needs_old_data = page_start < old_size
                && !full_page_write
                && (pos > page_start || pos + n < old_page_end);
            let page = if !needs_old_data {
                let mut pages = self.pages.lock();
                let (page, inserted) = if let Some(page) = pages.get(&page_idx) {
                    (page.clone(), false)
                } else {
                    let page = Arc::new(Mutex::new(Page::new_zeroed(Self::next_generation())));
                    pages.insert(page_idx, page.clone());
                    (page, true)
                };
                if inserted {
                    PAGE_CACHE_PAGE_COUNT.fetch_add(1, Ordering::Relaxed);
                }
                drop(pages);
                self.touch_page(page_idx, &page);
                if inserted {
                    Self::reclaim_global();
                }
                page
            } else {
                self.get_or_load(page_idx, lower)?
            };
            let mut p = page.lock();
            p.data[page_off..page_off + n].copy_from_slice(&buf[copied..copied + n]);
            p.dirty = true;
            drop(p);
            pos += n;
            copied += n;
        }
        Ok(buf.len())
    }

    pub fn mark_clean_range(&self, offset: usize, len: usize) {
        if len == 0 {
            return;
        }
        let end = offset.saturating_add(len);
        let start_page = offset / PAGE_SIZE;
        let end_page = end.div_ceil(PAGE_SIZE);
        let mut touched_pages = Vec::new();
        let pages = self.pages.lock();
        for page_idx in start_page..end_page {
            if let Some(page) = pages.get(&page_idx) {
                page.lock().dirty = false;
                touched_pages.push((page_idx, page.clone()));
            }
        }
        drop(pages);
        for (page_idx, page) in touched_pages {
            self.touch_page(page_idx, &page);
        }
        Self::reclaim_global();
    }

    /// 将脏页写回
    pub fn sync(&self, inode: &Arc<dyn InodeOp>, path: &str) -> SysResult {
        let file_size = *self.file_size.lock();
        let mut cleaned = false;
        let pages: Vec<_> = self
            .pages
            .lock()
            .iter()
            .map(|(&idx, p)| (idx, p.clone()))
            .collect();

        let mut run_offset = 0usize;
        let mut run_data = Vec::new();
        let mut run_pages: Vec<(usize, Arc<Mutex<Page>>)> = Vec::new();

        for (page_idx, page) in pages {
            let offset = page_idx * PAGE_SIZE;
            if offset >= file_size {
                continue;
            }

            let len = (file_size - offset).min(PAGE_SIZE);
            let p = page.lock();
            if !p.dirty {
                continue;
            }

            if !run_data.is_empty() && offset != run_offset + run_data.len() {
                let written = inode.write_at(path, run_offset, &run_data)?;
                if written != run_data.len() {
                    return Err(Errno::EIO);
                }
                for (idx, dirty_page) in run_pages.drain(..) {
                    dirty_page.lock().dirty = false;
                    self.touch_page(idx, &dirty_page);
                }
                run_data.clear();
                cleaned = true;
            }

            if run_data.is_empty() {
                run_offset = offset;
            }
            run_data.extend_from_slice(&p.data[..len]);
            drop(p);
            run_pages.push((page_idx, page));
        }

        if !run_data.is_empty() {
            let written = inode.write_at(path, run_offset, &run_data)?;
            if written != run_data.len() {
                return Err(Errno::EIO);
            }
            for (idx, dirty_page) in run_pages.drain(..) {
                dirty_page.lock().dirty = false;
                self.touch_page(idx, &dirty_page);
            }
            cleaned = true;
        }

        if cleaned {
            Self::reclaim_global();
        }
        Ok(())
    }
}

impl Drop for PageCache {
    fn drop(&mut self) {
        PAGE_CACHE_REGISTRY.lock().remove(&self.id);
        let page_count = self.pages.lock().len();
        if page_count != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_sub(page_count, Ordering::Relaxed);
        }
    }
}
