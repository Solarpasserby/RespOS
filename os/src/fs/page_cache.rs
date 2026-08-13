// os/src/fs/page_cache.rs

use super::vfs::{InodeOp, SuperBlockOp};
use crate::config::PAGE_CACHE_GLOBAL_MAX_PAGES;
use crate::config::PAGE_SIZE;
use crate::mm::{FrameTracker, frame_alloc};
use crate::syscall::{Errno, SysResult};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
#[cfg(feature = "debug_traces")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;

lazy_static! {
    static ref PAGE_CACHE_REGISTRY: Mutex<BTreeMap<usize, Weak<PageCache>>> =
        Mutex::new(BTreeMap::new());
    static ref PAGE_CACHE_LRU: Mutex<VecDeque<LruEntry>> = Mutex::new(VecDeque::new());
    /// Strong ownership for dirty regular-file state.  The inode cache itself
    /// is weak, so this registry—not File::drop—keeps an inode and its cache
    /// alive until both data and the corresponding timestamps are written.
    static ref DIRTY_OWNERS: Mutex<BTreeMap<usize, DirtyOwner>> = Mutex::new(BTreeMap::new());
}

static NEXT_PAGE_CACHE_ID: AtomicUsize = AtomicUsize::new(1);
static NEXT_DIRTY_OWNER_GENERATION: AtomicUsize = AtomicUsize::new(1);
static NEXT_LRU_GENERATION: AtomicUsize = AtomicUsize::new(1);
static NEXT_WRITEBACK_ID: AtomicUsize = AtomicUsize::new(1);
static PAGE_CACHE_PAGE_COUNT: AtomicUsize = AtomicUsize::new(0);
static PAGE_CACHE_DIRTY_PAGE_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "debug_traces")]
static WRITEBACK_FAULT_ARMED: AtomicBool = AtomicBool::new(false);

const WRITEBACK_BATCH_PAGES: usize = 32;
const DIRTY_PAGES_PER_CACHE_HIGH_WATERMARK: usize = 256;
const READ_AHEAD_PAGES: usize = 16;
const DIRTY_OWNER_HIGH_WATERMARK: usize = 128;
const BACKGROUND_WRITEBACK_OWNERS: usize = 8;

#[derive(Clone)]
struct DirtyOwner {
    cache: Arc<PageCache>,
    inode: Arc<dyn InodeOp>,
    filesystem: Arc<dyn SuperBlockOp>,
    path: String,
    background_failed: bool,
    generation: usize,
}

pub fn page_cache_page_count() -> usize {
    PAGE_CACHE_PAGE_COUNT.load(Ordering::Relaxed)
}

pub fn page_cache_dirty_page_count() -> usize {
    PAGE_CACHE_DIRTY_PAGE_COUNT.load(Ordering::Relaxed)
}

pub fn page_cache_registry_count() -> usize {
    PAGE_CACHE_REGISTRY.lock().len()
}

pub fn page_cache_lru_entry_count() -> usize {
    PAGE_CACHE_LRU.lock().len()
}

pub fn dirty_owner_count() -> usize {
    DIRTY_OWNERS.lock().len()
}

/// Publish the strong owner only after the page-cache mutation succeeded.
/// Re-registering refreshes the path used for lower I/O after rename/link.
pub fn register_dirty_owner(
    cache: Arc<PageCache>,
    inode: Arc<dyn InodeOp>,
    filesystem: Arc<dyn SuperBlockOp>,
    path: &str,
) {
    DIRTY_OWNERS.lock().insert(
        cache.id,
        DirtyOwner {
            cache,
            inode,
            filesystem,
            path: String::from(path),
            background_failed: false,
            generation: NEXT_DIRTY_OWNER_GENERATION.fetch_add(1, Ordering::Relaxed),
        },
    );
}

fn owner_needs_writeback(owner: &DirtyOwner) -> bool {
    owner.cache.has_dirty_pages() || owner.inode.has_pending_data_metadata()
}

pub fn release_clean_owner(cache: &Arc<PageCache>) {
    let mut owners = DIRTY_OWNERS.lock();
    if owners
        .get(&cache.id)
        .is_some_and(|owner| !owner_needs_writeback(owner))
    {
        owners.remove(&cache.id);
    }
}

fn sync_owner(owner: &DirtyOwner, range: Option<(usize, usize)>) -> SysResult {
    let data_result = match range {
        Some((start, end)) => owner
            .cache
            .sync_range(&owner.inode, owner.path.as_str(), start, end),
        None => owner.cache.sync(&owner.inode, owner.path.as_str()),
    };
    data_result?;

    // A range operation deliberately leaves inode-wide timestamps pending;
    // fsync/syncfs/unmount persist them only after all older dirty data.
    if range.is_none() {
        if let Err(error) = owner.inode.flush_data_metadata(owner.path.as_str()) {
            owner.cache.record_writeback_error(error);
            return Err(error);
        }
    }

    if !owner_needs_writeback(owner) {
        let mut owners = DIRTY_OWNERS.lock();
        if owners.get(&owner.cache.id).is_some_and(|current| {
            Arc::ptr_eq(&current.cache, &owner.cache) && !owner_needs_writeback(current)
        }) {
            owners.remove(&owner.cache.id);
        }
    }
    Ok(())
}

pub fn sync_page_cache_owner(cache: &Arc<PageCache>) -> SysResult {
    let owner = DIRTY_OWNERS.lock().get(&cache.id).cloned();
    if let Some(owner) = owner {
        sync_owner(&owner, None)
    } else {
        Ok(())
    }
}

pub fn sync_page_cache_range(cache: &Arc<PageCache>, start: usize, end: usize) -> SysResult {
    let owner = DIRTY_OWNERS.lock().get(&cache.id).cloned();
    if let Some(owner) = owner {
        sync_owner(&owner, Some((start, end)))
    } else {
        Ok(())
    }
}

pub fn sync_dirty_owners_for(filesystem: &Arc<dyn SuperBlockOp>) -> SysResult {
    let owners: Vec<_> = DIRTY_OWNERS
        .lock()
        .values()
        .filter(|owner| Arc::ptr_eq(&owner.filesystem, filesystem))
        .cloned()
        .collect();
    let mut first_error = None;
    for owner in owners {
        if let Err(error) = sync_owner(&owner, None) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub fn sync_all_dirty_owners() -> SysResult {
    let owners: Vec<_> = DIRTY_OWNERS.lock().values().cloned().collect();
    let mut first_error = None;
    for owner in owners {
        if let Err(error) = sync_owner(&owner, None) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Run a bounded amount of writeback at the syscall safe point.  Errors stay
/// attached to the PageCache and are reported by a later fsync/fdatasync;
/// they do not replace an unrelated syscall's result.
pub fn writeback_dirty_owners_if_needed() {
    if PAGE_CACHE_DIRTY_PAGE_COUNT.load(Ordering::Relaxed) < DIRTY_PAGES_PER_CACHE_HIGH_WATERMARK
        && DIRTY_OWNERS.lock().len() < DIRTY_OWNER_HIGH_WATERMARK
    {
        return;
    }
    let owners: Vec<_> = DIRTY_OWNERS
        .lock()
        .values()
        .filter(|owner| !owner.background_failed)
        .take(BACKGROUND_WRITEBACK_OWNERS)
        .cloned()
        .collect();
    for owner in owners {
        if sync_owner(&owner, None).is_err() {
            if let Some(current) = DIRTY_OWNERS.lock().get_mut(&owner.cache.id) {
                if current.generation == owner.generation {
                    current.background_failed = true;
                }
            }
        }
    }
}

#[cfg(feature = "debug_traces")]
pub fn arm_writeback_fault() {
    WRITEBACK_FAULT_ARMED.store(true, Ordering::Release);
}

#[cfg(feature = "debug_traces")]
fn take_writeback_fault() -> bool {
    WRITEBACK_FAULT_ARMED.swap(false, Ordering::AcqRel)
}

#[cfg(not(feature = "debug_traces"))]
fn take_writeback_fault() -> bool {
    false
}

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
    frame: Arc<FrameTracker>,
    dirty: bool,
    write_version: usize,
    /// The batch currently writing this page. Dirty may remain set when a
    /// concurrent writer changes the page after the batch took its snapshot.
    writeback: Option<usize>,
    /// Last failed writeback attempt for diagnostics. Observable error
    /// delivery is tracked per PageCache by `WritebackErrorState` below.
    writeback_error: Option<Errno>,
    generation: usize,
    queued: bool,
}

impl Page {
    fn new_zeroed(generation: usize) -> SysResult<Self> {
        Ok(Self {
            frame: Arc::new(frame_alloc().ok_or(Errno::ENOMEM)?),
            dirty: false,
            write_version: 0,
            writeback: None,
            writeback_error: None,
            generation,
            queued: false,
        })
    }

    fn bytes(&mut self) -> &mut [u8] {
        self.frame.ppn().get_bytes_array()
    }
}

#[derive(Clone, Copy)]
pub struct WritebackErrorCursor {
    sequence: usize,
}

struct WritebackErrorState {
    sequence: usize,
    error: Option<Errno>,
}

/// 共享页缓存，挂在 inode 上。内部用 Mutex 保护 BTreeMap，
/// I/O 在锁外完成，避免持锁期间做磁盘操作。
pub struct PageCache {
    id: usize,
    pages: Mutex<BTreeMap<usize, Arc<Mutex<Page>>>>,
    file_size: Mutex<usize>,
    /// 每次可见文件长度变化都递增。写回用它识别与 truncate/extend 的竞争。
    size_version: AtomicUsize,
    dirty_pages: AtomicUsize,
    writeback_error: Mutex<WritebackErrorState>,
    /// Serializes lower-file writeback against lower truncate. Buffered
    /// writers stay concurrent and are resolved by page/size generations.
    writeback_lock: Mutex<()>,
}

impl PageCache {
    pub fn new(file_size: usize) -> Arc<Self> {
        let id = NEXT_PAGE_CACHE_ID.fetch_add(1, Ordering::Relaxed);
        let cache = Arc::new(Self {
            id,
            pages: Mutex::new(BTreeMap::new()),
            file_size: Mutex::new(file_size),
            size_version: AtomicUsize::new(0),
            dirty_pages: AtomicUsize::new(0),
            writeback_error: Mutex::new(WritebackErrorState {
                sequence: 0,
                error: None,
            }),
            writeback_lock: Mutex::new(()),
        });
        PAGE_CACHE_REGISTRY
            .lock()
            .insert(id, Arc::downgrade(&cache));
        cache
    }

    pub fn len(&self) -> usize {
        *self.file_size.lock()
    }

    pub fn needs_writeback(&self) -> bool {
        let dirty = self.dirty_pages.load(Ordering::Relaxed);
        dirty >= DIRTY_PAGES_PER_CACHE_HIGH_WATERMARK
            || (dirty != 0
                && (PAGE_CACHE_DIRTY_PAGE_COUNT.load(Ordering::Relaxed)
                    > PAGE_CACHE_GLOBAL_MAX_PAGES / 2
                    || PAGE_CACHE_PAGE_COUNT.load(Ordering::Relaxed) > PAGE_CACHE_GLOBAL_MAX_PAGES))
    }

    pub fn has_dirty_pages(&self) -> bool {
        self.dirty_pages.load(Ordering::Relaxed) != 0
    }

    /// Start an open-file-description error cursor at the current sequence.
    /// Errors that predate open are not reported through the new descriptor.
    pub fn sample_writeback_error(&self) -> WritebackErrorCursor {
        WritebackErrorCursor {
            sequence: self.writeback_error.lock().sequence,
        }
    }

    /// Report the newest writeback error once to this open-file description.
    /// Duplicated descriptors share the cursor because they share `File`.
    pub fn check_writeback_error(&self, cursor: &mut WritebackErrorCursor) -> SysResult {
        let state = self.writeback_error.lock();
        if cursor.sequence == state.sequence {
            return Ok(());
        }
        cursor.sequence = state.sequence;
        state.error.map_or(Ok(()), Err)
    }

    fn record_writeback_error(&self, error: Errno) {
        let mut state = self.writeback_error.lock();
        state.sequence = state.sequence.wrapping_add(1);
        state.error = Some(error);
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
        // Examine each currently queued candidate at most once per pass.
        // Entries that are temporarily pinned by mmap are requeued for a
        // later pressure pass; bounding the scan prevents an all-pinned cache
        // from spinning forever.
        let scan_limit = PAGE_CACHE_LRU.lock().len();
        let mut scanned = 0usize;
        while scanned < scan_limit
            && PAGE_CACHE_PAGE_COUNT.load(Ordering::Relaxed) > PAGE_CACHE_GLOBAL_MAX_PAGES
        {
            let Some(entry) = PAGE_CACHE_LRU.lock().pop_front() else {
                break;
            };
            scanned += 1;
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
                    crate::perf::page_cache_eviction(1);
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
            // The page object itself is only owned by the cache and this
            // local lookup.  Its frame can additionally be owned by one or
            // more MAP_SHARED mappings; keep the cache entry in that case so
            // a later file read cannot load a second frame for the same page.
            if page_guard.dirty
                || Arc::strong_count(&page) != 2
                || Arc::strong_count(&page_guard.frame) != 1
            {
                drop(page_guard);
                drop(pages);
                self.touch_page(page_idx, &page);
                return ReclaimResult::Kept;
            }
        }
        pages.remove(&page_idx);
        ReclaimResult::Removed
    }

    pub fn resize(&self, new_size: usize) {
        let mut size = self.file_size.lock();
        if new_size == *size {
            return;
        }
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
                if let Some(page) = pages.remove(&victim) {
                    let mut page = page.lock();
                    // A MAP_SHARED mapping may still own the frame after the
                    // cache entry is removed.  Clear it so truncate followed
                    // by regrowth cannot expose the old file contents.
                    page.bytes().fill(0);
                    if page.dirty {
                        self.dirty_pages.fetch_sub(1, Ordering::Relaxed);
                        PAGE_CACHE_DIRTY_PAGE_COUNT.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }
            if new_size % PAGE_SIZE != 0 {
                let last_page_idx = new_size / PAGE_SIZE;
                if let Some(page) = pages.get(&last_page_idx) {
                    let mut page = page.lock();
                    page.bytes()[new_size % PAGE_SIZE..].fill(0);
                    // 尾页内容也是写回快照的一部分；即使只是清零不可见尾部，
                    // 也必须使并发写回的旧快照失效。
                    page.write_version = page.write_version.wrapping_add(1);
                }
            }
        }
        if removed_pages != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_sub(removed_pages, Ordering::Relaxed);
        }
        *size = new_size;
        self.size_version.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn with_writeback_exclusion<T>(
        &self,
        operation: impl FnOnce() -> SysResult<T>,
    ) -> SysResult<T> {
        let _guard = self.writeback_lock.lock();
        operation()
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
        read_ahead_pages: usize,
    ) -> SysResult<Arc<Mutex<Page>>> {
        if let Some(page) = self.lookup_page(page_idx) {
            crate::perf::page_cache_hit(1);
            return Ok(page);
        }
        crate::perf::page_cache_miss(1);

        let load_version = self.size_version.load(Ordering::Acquire);
        let file_size = *self.file_size.lock();
        let page_start = page_idx * PAGE_SIZE;
        let available_pages = file_size.div_ceil(PAGE_SIZE).saturating_sub(page_idx);
        let mut load_pages = if lower.is_some() && page_start < file_size {
            read_ahead_pages
                .clamp(1, READ_AHEAD_PAGES)
                .min(available_pages)
        } else {
            1
        };

        // Do not read through a page that another access has already
        // published. Besides wasting lower I/O, an overlapping run allocates
        // and clears frames that are discarded when candidates are published.
        // The requested page was checked above; only bound speculative pages.
        if load_pages > 1 {
            let pages = self.pages.lock();
            if let Some(first_cached) =
                (1..load_pages).find(|run_idx| pages.contains_key(&(page_idx + run_idx)))
            {
                load_pages = first_cached;
            }
        }

        // Read a sequential run while outside the PageCache lock.  One 64 KiB
        // lwext4 operation replaces up to sixteen 4 KiB operations and lets
        // the lower block layer preserve its multi-block request batching.
        let read_len = if lower.is_some() && page_start < file_size {
            (file_size - page_start).min(load_pages * PAGE_SIZE)
        } else {
            0
        };
        let mut read_buf = Vec::new();
        let did_lower_fill = read_len != 0;
        if read_len != 0 {
            read_buf
                .try_reserve_exact(read_len)
                .map_err(|_| Errno::ENOMEM)?;
            read_buf.resize(read_len, 0);
            if let Some((inode, path)) = lower {
                crate::perf::page_cache_fill_call(1);
                crate::perf::page_cache_fill_bytes(read_len);
                crate::perf::page_cache_fill_candidate_pages(load_pages);
                match inode.read_at(path, page_start, &mut read_buf) {
                    Ok(_) | Err(Errno::ENOENT) => {}
                    Err(err) => return Err(err),
                }
            }
        }

        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(load_pages)
            .map_err(|_| Errno::ENOMEM)?;
        for run_idx in 0..load_pages {
            let mut new_page = Page::new_zeroed(Self::next_generation())?;
            let source_start = run_idx * PAGE_SIZE;
            if source_start < read_buf.len() {
                let source_end = (source_start + PAGE_SIZE).min(read_buf.len());
                new_page.bytes()[..source_end - source_start]
                    .copy_from_slice(&read_buf[source_start..source_end]);
            }
            candidates.push((page_idx + run_idx, Arc::new(Mutex::new(new_page)), false));
        }

        let mut pages = self.pages.lock();
        if self.size_version.load(Ordering::Acquire) != load_version {
            drop(pages);
            return self.get_or_load(page_idx, lower, read_ahead_pages);
        }
        let mut requested = None;
        let mut inserted_count = 0usize;
        for (candidate_idx, candidate, inserted) in candidates.iter_mut() {
            let page = if let Some(existing) = pages.get(candidate_idx) {
                existing.clone()
            } else {
                pages.insert(*candidate_idx, candidate.clone());
                *inserted = true;
                inserted_count += 1;
                candidate.clone()
            };
            if *candidate_idx == page_idx {
                requested = Some(page);
            }
        }
        drop(pages);
        if did_lower_fill {
            crate::perf::page_cache_fill_published_pages(inserted_count);
            crate::perf::page_cache_fill_raced_pages(load_pages - inserted_count);
        }
        if inserted_count != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_add(inserted_count, Ordering::Relaxed);
            for (candidate_idx, candidate, inserted) in candidates.iter() {
                if *inserted {
                    self.touch_page(*candidate_idx, candidate);
                }
            }
            Self::reclaim_global();
        }
        let requested = requested.ok_or(Errno::EIO)?;
        self.touch_page(page_idx, &requested);
        Ok(requested)
    }

    /// Return the physical cache page used by a shared file mapping.
    ///
    /// Keeping the frame inside `PageCache` makes buffered I/O and
    /// `MAP_SHARED` observe the same bytes without a second frame or a
    /// coherence overlay.  Global reclaim detects the additional frame owner
    /// and retains the page metadata until the mapping is gone.
    pub(crate) fn shared_frame_at(
        &self,
        page_idx: usize,
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
    ) -> SysResult<Arc<FrameTracker>> {
        let page = self.get_or_load(page_idx, lower, READ_AHEAD_PAGES)?;
        let frame = page.lock().frame.clone();
        Ok(frame)
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
            let page = self.get_or_load(page_idx, lower, READ_AHEAD_PAGES)?;
            let mut p = page.lock();
            buf[copied..copied + n].copy_from_slice(&p.bytes()[page_off..page_off + n]);
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
                self.size_version.fetch_add(1, Ordering::Release);
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
                    let page = Arc::new(Mutex::new(Page::new_zeroed(Self::next_generation())?));
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
                self.get_or_load(page_idx, lower, 1)?
            };
            let mut p = page.lock();
            p.bytes()[page_off..page_off + n].copy_from_slice(&buf[copied..copied + n]);
            if !p.dirty {
                self.dirty_pages.fetch_add(1, Ordering::Relaxed);
                let global_dirty = PAGE_CACHE_DIRTY_PAGE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                crate::perf::observe_dirty_pages(global_dirty);
            }
            p.dirty = true;
            p.write_version = p.write_version.wrapping_add(1);
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
                let mut guard = page.lock();
                if guard.dirty {
                    guard.dirty = false;
                    self.dirty_pages.fetch_sub(1, Ordering::Relaxed);
                    PAGE_CACHE_DIRTY_PAGE_COUNT.fetch_sub(1, Ordering::Relaxed);
                }
                drop(guard);
                touched_pages.push((page_idx, page.clone()));
            }
        }
        drop(pages);
        for (page_idx, page) in touched_pages {
            self.touch_page(page_idx, &page);
        }
        Self::reclaim_global();
    }

    fn finish_writeback_batch(
        snapshots: &[(Arc<Mutex<Page>>, usize)],
        writeback_id: usize,
        error: Option<Errno>,
    ) {
        for (page, _) in snapshots {
            let mut page = page.lock();
            if page.writeback == Some(writeback_id) {
                page.writeback = None;
                page.writeback_error = error;
            }
        }
    }

    /// 将脏页写回。失败保留 dirty，并发布给所有已打开文件的错误游标。
    pub fn sync(&self, inode: &Arc<dyn InodeOp>, path: &str) -> SysResult {
        let result = self.sync_inner(inode, path, None);
        if let Err(error) = result {
            self.record_writeback_error(error);
        }
        result
    }

    /// Write all dirty pages intersecting [start, end).  Page granularity is
    /// intentional: storage writeback never splits the PageCache identity.
    pub fn sync_range(
        &self,
        inode: &Arc<dyn InodeOp>,
        path: &str,
        start: usize,
        end: usize,
    ) -> SysResult {
        let result = self.sync_inner(inode, path, Some((start, end)));
        if let Err(error) = result {
            self.record_writeback_error(error);
        }
        result
    }

    fn sync_inner(
        &self,
        inode: &Arc<dyn InodeOp>,
        path: &str,
        range: Option<(usize, usize)>,
    ) -> SysResult {
        let _writeback_guard = self.writeback_lock.lock();
        let file_size = *self.file_size.lock();
        let size_version = self.size_version.load(Ordering::Acquire);
        let pages: Vec<_> = self
            .pages
            .lock()
            .iter()
            .filter(|(idx, _)| {
                range.map_or(true, |(start, end)| {
                    let page_start = **idx * PAGE_SIZE;
                    page_start < end && page_start.saturating_add(PAGE_SIZE) > start
                })
            })
            .map(|(&idx, p)| (idx, p.clone()))
            .collect();

        let mut cursor = 0usize;
        let mut cleaned = false;
        let mut cleaned_pages = Vec::new();

        while cursor < pages.len() {
            while cursor < pages.len() && !pages[cursor].1.lock().dirty {
                cursor += 1;
            }
            if cursor == pages.len() {
                break;
            }

            let first_page_idx = pages[cursor].0;
            if first_page_idx * PAGE_SIZE >= file_size {
                break;
            }

            let mut expected_page_idx = first_page_idx;
            let mut snapshots = Vec::new();
            let mut data = Vec::new();
            let writeback_id = NEXT_WRITEBACK_ID.fetch_add(1, Ordering::Relaxed);

            while cursor < pages.len() && snapshots.len() < WRITEBACK_BATCH_PAGES {
                let (page_idx, page) = &pages[cursor];
                if *page_idx != expected_page_idx {
                    break;
                }
                let page_offset = page_idx * PAGE_SIZE;
                if page_offset >= file_size {
                    break;
                }
                let page_len = (file_size - page_offset).min(PAGE_SIZE);
                let mut page_guard = page.lock();
                if !page_guard.dirty {
                    break;
                }
                data.extend_from_slice(&page_guard.bytes()[..page_len]);
                page_guard.writeback = Some(writeback_id);
                page_guard.writeback_error = None;
                snapshots.push((page.clone(), page_guard.write_version));
                drop(page_guard);
                expected_page_idx += 1;
                cursor += 1;
            }

            let offset = first_page_idx * PAGE_SIZE;
            // A feature-gated one-shot fault keeps the release path free of a
            // control surface while allowing the error/cursor protocol to be
            // exercised deterministically in a disposable test guest.
            if take_writeback_fault() {
                Self::finish_writeback_batch(&snapshots, writeback_id, Some(Errno::EIO));
                return Err(Errno::EIO);
            }
            let written = match inode.write_at(path, offset, &data) {
                Ok(written) => written,
                Err(error) => {
                    Self::finish_writeback_batch(&snapshots, writeback_id, Some(error));
                    return Err(error);
                }
            };
            if written != data.len() {
                Self::finish_writeback_batch(&snapshots, writeback_id, Some(Errno::EIO));
                return Err(Errno::EIO);
            }
            crate::perf::page_cache_writeback_bytes(written);

            // truncate 可以与另一个 File 的 fsync 并发。旧长度的写回若越过新 EOF，
            // 会重新扩展底层文件；检测到长度代次变化后立即恢复当前长度，并保留
            // 所有 dirty/version 状态供下一次 fsync 重试。
            if self.size_version.load(Ordering::Acquire) != size_version {
                let current_size = *self.file_size.lock();
                if offset.saturating_add(written) > current_size {
                    if let Err(error) = inode.truncate(path, current_size) {
                        Self::finish_writeback_batch(&snapshots, writeback_id, Some(error));
                        return Err(error);
                    }
                }
                Self::finish_writeback_batch(&snapshots, writeback_id, None);
                continue;
            }

            for (page_offset, (page, version)) in snapshots.into_iter().enumerate() {
                let mut page_guard = page.lock();
                if page_guard.writeback != Some(writeback_id) {
                    continue;
                }
                page_guard.writeback = None;
                page_guard.writeback_error = None;
                if page_guard.dirty && page_guard.write_version == version {
                    page_guard.dirty = false;
                    self.dirty_pages.fetch_sub(1, Ordering::Relaxed);
                    PAGE_CACHE_DIRTY_PAGE_COUNT.fetch_sub(1, Ordering::Relaxed);
                    cleaned_pages.push((first_page_idx + page_offset, page.clone()));
                    cleaned = true;
                }
            }
        }

        if cleaned {
            for (page_idx, page) in cleaned_pages {
                self.touch_page(page_idx, &page);
            }
            Self::reclaim_global();
        }
        Ok(())
    }
}

impl Drop for PageCache {
    fn drop(&mut self) {
        PAGE_CACHE_REGISTRY.lock().remove(&self.id);
        // LRU entries own no page data, but leaving one entry behind for every
        // page of every short-lived file makes the global VecDeque grow
        // without bound. Remove this cache's entries while its id is still
        // unambiguous. This leaves the queue proportional to live resident
        // pages instead of cumulative file traffic.
        PAGE_CACHE_LRU
            .lock()
            .retain(|entry| entry.cache_id != self.id);
        let page_count = self.pages.lock().len();
        let dirty_count = self.dirty_pages.load(Ordering::Relaxed);
        if page_count != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_sub(page_count, Ordering::Relaxed);
        }
        if dirty_count != 0 {
            PAGE_CACHE_DIRTY_PAGE_COUNT.fetch_sub(dirty_count, Ordering::Relaxed);
        }
    }
}
