// os/src/fs/page_cache.rs

//! 常规文件页缓存、全局回收与写回状态机。
//!
//! 每个稳定 inode 拥有一个 `PageCache`，buffered I/O 与 file-backed mmap 从中取得同一
//! frame。cache entry 需要跟踪 dirty、writeback、映射 pin、backing reservation 和 LRU
//! 世代；“页在缓存中”不等于“可以立即驱逐”。
//!
//! 锁与 I/O 边界是本模块的核心：选择和标记待写回页时持锁，真正 lower inode I/O 在锁外
//! 执行，完成后再提交状态。dirty 位必须在写回启动前清到正确世代，写回期间的新写入不能被
//! 旧完成事件误清除。驱逐仅允许 clean、unmapped、unpinned 且不在 writeback 的页。
//!
//! `DIRTY_OWNERS` 为未持久化数据和时间戳提供强生命周期，不能依赖 inode 弱缓存或 `File::drop`。
//! DONTNEED、truncate、fsync 和全局 reclaim 都应复用相同状态机，而不是各自直接删除 frame。

use super::vfs::{InodeOp, SuperBlockOp};
use crate::config::PAGE_CACHE_GLOBAL_MAX_PAGES;
use crate::config::PAGE_SIZE;
use crate::mm::{frame_alloc, FrameTracker};
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
    /// dirty 常规文件状态的强所有权。inode cache 本身只保存弱引用，因此由这个注册表而非
    /// `File::drop` 保持 inode 及其 cache 存活，直到数据与对应时间戳都完成写回。
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
pub(crate) const DEFAULT_READ_AHEAD_PAGES: usize = 16;
pub(crate) const SEQUENTIAL_READ_AHEAD_PAGES: usize = DEFAULT_READ_AHEAD_PAGES * 2;
const MAX_READ_AHEAD_PAGES: usize = SEQUENTIAL_READ_AHEAD_PAGES;
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

/// 仅在页缓存修改成功后发布强 owner；rename/link 后重新注册可刷新 lower I/O 使用的路径。
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

    // range 操作刻意保留 inode-wide 时间戳为 pending；fsync/syncfs/unmount 会在所有更早的
    // dirty data 之后持久化它们。
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

/// 在 syscall safe point 执行有界写回。错误继续附着在 PageCache 上，由后续
/// fsync/fdatasync 报告，不能替换无关 syscall 的返回值。
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
    /// 当前正在写这个页面的 batch。batch 取得快照后若有并发 writer 修改页面，dirty 可继续保持。
    writeback: Option<usize>,
    /// 最近一次失败的写回尝试，仅供诊断；可观察错误由下方每个 PageCache 的
    /// `WritebackErrorState` 跟踪和投递。
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
    /// 每个文件页中已经由 page_mkwrite 建立持久 backing 的 prefix 长度。当文件系统 block
    /// 小于 VM page，且 ftruncate 扩展 resident tail page 时需要该信息。
    mmap_reserved_prefixes: Mutex<BTreeMap<usize, usize>>,
    /// 串行化 lower-file writeback 与 lower truncate；buffered writer 仍可并发，并由
    /// page/size generation 解决竞态。
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
            mmap_reserved_prefixes: Mutex::new(BTreeMap::new()),
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

    pub fn reserve_mmap_prefix<T>(
        &self,
        page_idx: usize,
        prefix_len: usize,
        reserve: impl FnOnce(usize) -> SysResult<T>,
    ) -> SysResult<Option<T>> {
        let mut prefixes = self.mmap_reserved_prefixes.lock();
        let current = prefixes.get(&page_idx).copied().unwrap_or(0);
        if prefix_len <= current {
            return Ok(None);
        }
        let result = reserve(current)?;
        prefixes.insert(page_idx, prefix_len);
        Ok(Some(result))
    }

    /// 从当前 sequence 创建 open-file-description 的错误游标；open 之前的错误不通过新描述符报告。
    pub fn sample_writeback_error(&self) -> WritebackErrorCursor {
        WritebackErrorCursor {
            sequence: self.writeback_error.lock().sequence,
        }
    }

    /// 向该 open-file description 至多报告一次最新写回错误。dup 描述符共享 `File`，因此也共享游标。
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
        // 每轮至多检查当前队列中的每个候选一次。被 mmap 临时 pin 的条目重新入队，留待后续
        // pressure pass；限制扫描次数可避免全被 pin 的 cache 永久自旋。
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
            // page object 本身只由 cache 与此次局部 lookup 持有，但其 frame 还可能被一个或多个
            // MAP_SHARED mapping 持有。此时保留 cache entry，避免后续 file read 为同一页加载第二个 frame。
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

    /// 更新缓存的逻辑文件长度，并在缩小时裁剪 EOF 外页面和末页内容。
    ///
    /// 操作递增 size generation，使并发装入/写回识别过期快照。整页位于新 EOF 后时从 map
    /// 移除；边界页只清零不可见尾部并裁剪持久后端 reservation，防止 truncate 后 regrow
    /// 暴露旧数据。dirty/LRU/全局页计数必须与实际移除页同步，仍被 mmap pin 的帧由 Arc
    /// 生命周期延后释放，但不能继续作为当前文件缓存命中。
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
                    // cache entry 删除后 MAP_SHARED mapping 仍可能持有 frame。必须清零，避免
                    // truncate 后重新扩容暴露旧文件内容。
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
            let mut prefixes = self.mmap_reserved_prefixes.lock();
            prefixes.retain(|page_idx, prefix| {
                let page_start = page_idx.saturating_mul(PAGE_SIZE);
                if page_start >= new_size {
                    return false;
                }
                *prefix = (*prefix).min(new_size - page_start);
                *prefix != 0
            });
        }
        if removed_pages != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_sub(removed_pages, Ordering::Relaxed);
        }
        *size = new_size;
        self.size_version.fetch_add(1, Ordering::Release);
    }

    /// 把 lower filesystem 已成功完成的 hole punch 发布到共享页缓存。完整覆盖页直接丢弃；
    /// 部分边界页保持 cache coherent，只清零请求范围。MAP_PRIVATE COW frame 是独立分配，
    /// 因而保持不变。
    pub fn punch_hole(&self, offset: usize, len: usize) {
        if len == 0 {
            return;
        }
        let file_size = *self.file_size.lock();
        let end = offset.saturating_add(len).min(file_size);
        if offset >= end {
            return;
        }

        let start_page = offset / PAGE_SIZE;
        let end_page = end.div_ceil(PAGE_SIZE);
        let mut removed_pages = 0usize;
        let mut pages = self.pages.lock();
        for page_idx in start_page..end_page {
            let page_start = page_idx.saturating_mul(PAGE_SIZE);
            let zero_start = offset.saturating_sub(page_start).min(PAGE_SIZE);
            let zero_end = end.saturating_sub(page_start).min(PAGE_SIZE);
            if zero_start >= zero_end {
                continue;
            }
            let fully_covered = zero_start == 0 && zero_end == PAGE_SIZE;
            if fully_covered {
                if let Some(page) = pages.remove(&page_idx) {
                    let mut page = page.lock();
                    page.bytes().fill(0);
                    page.write_version = page.write_version.wrapping_add(1);
                    if page.dirty {
                        page.dirty = false;
                        self.dirty_pages.fetch_sub(1, Ordering::Relaxed);
                        PAGE_CACHE_DIRTY_PAGE_COUNT.fetch_sub(1, Ordering::Relaxed);
                    }
                    removed_pages += 1;
                }
            } else if let Some(page) = pages.get(&page_idx) {
                let mut page = page.lock();
                page.bytes()[zero_start..zero_end].fill(0);
                page.write_version = page.write_version.wrapping_add(1);
            }
        }
        drop(pages);
        if removed_pages != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_sub(removed_pages, Ordering::Relaxed);
        }
        // 并发写回快照必须观察到文件内容经历了破坏性修改，即使逻辑 size 没有改变。
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
    fn lookup_page(&self, page_idx: usize, mark_accessed: bool) -> Option<Arc<Mutex<Page>>> {
        let page = self.pages.lock().get(&page_idx).cloned();
        if mark_accessed {
            if let Some(page) = page.as_ref() {
                self.touch_page(page_idx, page);
            }
        }
        page
    }

    /// 获取缓存页，并在缺失时于 PageCache 锁外完成成批惰性装入。
    ///
    /// 命中直接返回；未命中时先快照文件长度代次，根据 advice 选择预读窗口，并在遇到
    /// 已缓存页面前截断 speculative run。下层 I/O 和页帧分配均不持有 `pages` 锁，避免
    /// 把慢速块设备操作带入全局缓存临界区。I/O 全部成功后才竞争发布候选页，失败不会
    /// 留下看似有效的零页。
    ///
    /// 发布时若文件长度代次已变化，必须丢弃候选并重新装入，防止 truncate 并发后发布
    /// 旧 EOF 内容；若另一线程抢先插入同一页，则复用获胜页面。返回值中的布尔量表示本次
    /// 是否实际执行了下层填充，供 major/minor fault 记账使用。
    fn get_or_load_with_status(
        &self,
        page_idx: usize,
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
        read_ahead_pages: usize,
        mark_accessed: bool,
    ) -> SysResult<(Arc<Mutex<Page>>, bool)> {
        if let Some(page) = self.lookup_page(page_idx, mark_accessed) {
            crate::perf::page_cache_hit(1);
            return Ok((page, false));
        }
        crate::perf::page_cache_miss(1);

        let load_version = self.size_version.load(Ordering::Acquire);
        let file_size = *self.file_size.lock();
        let page_start = page_idx * PAGE_SIZE;
        let available_pages = file_size.div_ceil(PAGE_SIZE).saturating_sub(page_idx);
        let mut load_pages = if lower.is_some() && page_start < file_size {
            read_ahead_pages
                .clamp(1, MAX_READ_AHEAD_PAGES)
                .min(available_pages)
        } else {
            1
        };

        // 不要越过另一访问已发布的页面继续读取。重叠 run 不仅浪费 lower I/O，还会分配并
        // 清零最终在发布候选时被丢弃的 frame。请求页已在上方检查，这里只限制 speculative page。
        if load_pages > 1 {
            let pages = self.pages.lock();
            if let Some(first_cached) =
                (1..load_pages).find(|run_idx| pages.contains_key(&(page_idx + run_idx)))
            {
                load_pages = first_cached;
            }
        }

        // 在 PageCache 锁外顺序读取一段。NORMAL 最多合并 64 KiB，SEQUENTIAL 最多 128 KiB，
        // 使 lower block layer 能继续保留多 block request batching。
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
            return self.get_or_load_with_status(page_idx, lower, read_ahead_pages, mark_accessed);
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
        if mark_accessed {
            self.touch_page(page_idx, &requested);
        }
        Ok((requested, did_lower_fill))
    }

    fn get_or_load(
        &self,
        page_idx: usize,
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
        read_ahead_pages: usize,
        mark_accessed: bool,
    ) -> SysResult<Arc<Mutex<Page>>> {
        self.get_or_load_with_status(page_idx, lower, read_ahead_pages, mark_accessed)
            .map(|(page, _)| page)
    }

    /// 返回共享文件映射使用的物理缓存页。
    ///
    /// frame 留在 `PageCache` 中，使 buffered I/O 与 `MAP_SHARED` 无需第二个 frame 或额外
    /// coherence overlay 就能观察相同字节。全局回收会发现额外 frame owner，并保留页面
    /// metadata 直到 mapping 消失。
    pub(crate) fn shared_frame_at(
        &self,
        page_idx: usize,
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
        read_ahead_pages: usize,
        mark_accessed: bool,
    ) -> SysResult<(Arc<FrameTracker>, bool)> {
        let (page, did_lower_fill) =
            self.get_or_load_with_status(page_idx, lower, read_ahead_pages, mark_accessed)?;
        let frame = page.lock().frame.clone();
        Ok((frame, did_lower_fill))
    }

    /// 从页缓存读取数据到 buf
    pub fn read_at(
        &self,
        offset: usize,
        buf: &mut [u8],
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
    ) -> SysResult<usize> {
        self.read_at_advised(offset, buf, lower, DEFAULT_READ_AHEAD_PAGES, true)
    }

    pub fn read_at_advised(
        &self,
        offset: usize,
        buf: &mut [u8],
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
        read_ahead_pages: usize,
        mark_accessed: bool,
    ) -> SysResult<usize> {
        let file_size = *self.file_size.lock();
        let mut copied = 0;
        let mut pos = offset.min(file_size);
        let end = file_size.min(offset.saturating_add(buf.len()));
        while pos < end {
            let page_idx = pos / PAGE_SIZE;
            let page_off = pos % PAGE_SIZE;
            let n = (end - pos).min(PAGE_SIZE - page_off);
            let page = self.get_or_load(page_idx, lower, read_ahead_pages, mark_accessed)?;
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
                self.get_or_load(page_idx, lower, 1, true)?
            };
            let mut p = page.lock();
            p.bytes()[page_off..page_off + n].copy_from_slice(&buf[copied..copied + n]);
            if !p.dirty {
                self.dirty_pages.fetch_add(1, Ordering::Relaxed);
                let global_dirty = PAGE_CACHE_DIRTY_PAGE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                crate::perf::observe_dirty_pages(global_dirty);
                if lower.is_some() {
                    if let Some(task) = crate::task::current_task() {
                        task.note_output_blocks(PAGE_SIZE / crate::config::BLOCK_SIZE);
                    }
                }
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

    /// `force_page_cache_readahead()` 的 best-effort 同步版本。fadvise 调用方刻意忽略错误，
    /// 因为 Linux 不会把 WILLNEED 的读取失败变成 syscall 返回错误。
    pub fn prefetch_range(
        &self,
        offset: usize,
        len: usize,
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
        read_ahead_pages: usize,
    ) {
        let file_size = *self.file_size.lock();
        let end = if len == 0 {
            file_size
        } else {
            offset.saturating_add(len).min(file_size)
        };
        let start_page = offset / PAGE_SIZE;
        let end_page = end.div_ceil(PAGE_SIZE);
        for page_idx in start_page..end_page {
            if self
                .get_or_load(page_idx, lower, read_ahead_pages, true)
                .is_err()
            {
                break;
            }
        }
    }

    /// 丢弃 advice 字节范围完整覆盖的 clean、unmapped cache page。范围到达 EOF 时，末尾
    /// 部分页也可安全丢弃，以匹配 Linux `generic_fadvise()`。
    pub fn evict_clean_range(&self, offset: usize, len: usize) {
        let file_size = *self.file_size.lock();
        let range_end = if len == 0 {
            usize::MAX
        } else {
            offset.saturating_add(len)
        };
        let start_page = offset.div_ceil(PAGE_SIZE);
        let end_page = if range_end >= file_size {
            file_size.div_ceil(PAGE_SIZE)
        } else {
            range_end / PAGE_SIZE
        };
        if end_page <= start_page {
            return;
        }
        let mut pages = self.pages.lock();
        let victims = pages
            .range(start_page..end_page)
            .map(|(&page_idx, _)| page_idx)
            .collect::<Vec<_>>();
        let mut removed = 0usize;
        for page_idx in victims {
            let removable = pages.get(&page_idx).is_some_and(|page| {
                let page_guard = page.lock();
                !page_guard.dirty
                    && Arc::strong_count(page) == 1
                    && Arc::strong_count(&page_guard.frame) == 1
            });
            if removable {
                pages.remove(&page_idx);
                removed += 1;
            }
        }
        drop(pages);
        if removed != 0 {
            PAGE_CACHE_PAGE_COUNT.fetch_sub(removed, Ordering::Relaxed);
        }
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

    /// 写回所有与 `[start, end)` 相交的脏页。刻意采用页粒度，因为存储写回不会拆分
    /// 页缓存对象的身份边界。
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

    /// 把全部或指定范围内的脏页按连续批次写回下层 inode。
    ///
    /// `writeback_lock` 将 truncate、hole-punch 与其他写回串行化；函数先快照候选 Arc，
    /// 再按 `WRITEBACK_BATCH_PAGES` 合并连续脏页。每页记录 writeback id 与写版本，只有
    /// 下层完整写入且写版本未在期间变化时才能清除 dirty，因而并发用户写不会被误认为
    /// 已持久化。短写统一视为 EIO，并把错误保留给每个打开文件描述的错误游标。
    ///
    /// 若写回期间文件长度代次变化，旧批次可能越过新的 EOF；此时恢复当前下层长度并
    /// 保留脏状态供下一次重试。调用者负责在需要 `fsync` 语义时另行执行文件系统持久化屏障。
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
            // feature 控制的一次性 fault 不给 release 路径增加控制面，同时允许在一次性测试
            // guest 中确定性验证 error/cursor 协议。
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
        // LRU entry 不拥有页面数据，但若每个短命文件的每页都残留一个 entry，全局 VecDeque
        // 会无界增长。在 cache id 仍唯一时删除其条目，使队列规模与 live resident page
        // 成比例，而不是与累计文件流量成比例。
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
