// os/src/fs/page_cache.rs

use super::vfs::InodeOp;
use crate::config::PAGE_SIZE;
use crate::syscall::{Errno, SysResult};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// 页缓存中的一页
pub struct Page {
    data: Vec<u8>, // PAGE_SIZE 字节
    dirty: bool,
}

impl Page {
    fn new_zeroed() -> Self {
        Self {
            data: alloc::vec![0u8; PAGE_SIZE],
            dirty: false,
        }
    }
}

/// 共享页缓存，挂在 inode 上。内部用 Mutex 保护 BTreeMap，
/// I/O 在锁外完成，避免持锁期间做磁盘操作。
pub struct PageCache {
    pages: Mutex<BTreeMap<usize, Arc<Mutex<Page>>>>,
    file_size: Mutex<usize>,
}

impl PageCache {
    pub fn new(file_size: usize) -> Self {
        Self {
            pages: Mutex::new(BTreeMap::new()),
            file_size: Mutex::new(file_size),
        }
    }

    pub fn len(&self) -> usize {
        *self.file_size.lock()
    }

    pub fn resize(&self, new_size: usize) {
        let mut size = self.file_size.lock();
        if new_size < *size {
            let mut pages = self.pages.lock();
            // 文件缩小时，删除所有超范围的页。
            pages.retain(|&idx, _| idx * PAGE_SIZE < new_size);
            if new_size % PAGE_SIZE != 0 {
                let last_page_idx = new_size / PAGE_SIZE;
                if let Some(page) = pages.get(&last_page_idx) {
                    let mut page = page.lock();
                    page.data[new_size % PAGE_SIZE..].fill(0);
                }
            }
        }
        *size = new_size;
    }

    /// 查 BTreeMap 获取页（不触发 I/O）
    fn lookup_page(&self, page_idx: usize) -> Option<Arc<Mutex<Page>>> {
        self.pages.lock().get(&page_idx).cloned()
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
        let mut new_page = Page::new_zeroed();

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
            return Ok(existing.clone());
        }
        pages.insert(page_idx, page.clone());
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
                pages
                    .entry(page_idx)
                    .or_insert_with(|| Arc::new(Mutex::new(Page::new_zeroed())))
                    .clone()
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

    /// 将脏页写回
    pub fn sync(&self, inode: &Arc<dyn InodeOp>, path: &str) -> SysResult {
        let file_size = *self.file_size.lock();
        let pages: Vec<_> = self
            .pages
            .lock()
            .iter()
            .map(|(&idx, p)| (idx, p.clone()))
            .collect();
        for (page_idx, page) in pages {
            let mut p = page.lock();
            if p.dirty {
                let offset = page_idx * PAGE_SIZE;
                if offset >= file_size {
                    continue;
                }
                let len = (file_size - offset).min(PAGE_SIZE);
                inode.write_at(path, offset, &p.data[..len])?;
                p.dirty = false;
            }
        }
        Ok(())
    }
}
