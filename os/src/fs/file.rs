// os/src/vfs/file.rs

//! 打开的常规文件对象及 open-file 级状态。
//!
//! `File` 持有 inode，并在 `FileInner` 中保存当前 offset、open flags、readdir 快照、fadvise
//! 和 writeback error cursor 等“open-file description”状态。因此 dup 和 fork 共享这些字段，
//! 同一路径再次 open 则获得独立状态；不要把它们错误地下沉到 inode 或上移到 fd entry。
//!
//! buffered read/write、pread/pwrite 与 file-backed mmap 必须通过同一 PageCache 保持数据
//! 一致。扩容前先经过 mount geometry/容量检查，写成功后再更新时间戳和 offset；`O_APPEND`
//! 的 EOF 选择与整次写入要在同一原子协议中。writeback 错误通过每个 open-file cursor
//! 报告，不能因另一个 fd 已观察错误而全局清除。
//!
//! `File` 的 Drop 不是可靠持久化边界。dirty owner 和 superblock sync 负责在最后一个 fd
//! 消失后继续保有待写回状态。

use super::vfs::{InodeOp, InodeType, LinuxDirent64, SuperBlockOp};
use crate::config::{KERNEL_HEAP_SIZE, PAGE_SIZE};
use crate::fs::ext4::Ext4Inode;
use crate::fs::mount::{
    MS_LAZYTIME, MS_NOATIME, MS_NODIRATIME, MS_STRICTATIME,
    check_mount_allocation_available, check_mount_file_growth, mount_block_size,
};
use crate::fs::page_cache::{
    DEFAULT_READ_AHEAD_PAGES, PageCache, SEQUENTIAL_READ_AHEAD_PAGES, WritebackErrorCursor,
    register_dirty_owner, sync_page_cache_owner, sync_page_cache_range,
};
use crate::fs::{KStat, Path, PollEvents};
use crate::mm::FrameTracker;
use crate::syscall::{Errno, SysResult};
use crate::timer::TimeSpec;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

/// 常规文件的 open-file 对象；`inner` 中的状态由 dup/fork 共享。
pub struct File {
    inode: Arc<dyn InodeOp>,
    shared_page_identity: Option<(u64, u64)>,
    inner: Mutex<FileInner>,
}

#[derive(Clone, Copy)]
pub struct TmpFileMeta {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

struct FileInner {
    offset: usize,
    path: Arc<Path>,
    flags: OpenFlags,
    /// 当前目录流遍历所缓存的目录项；getdents 每次从偏移 0 开始时都会重建。
    dirent_cache: Option<Arc<Vec<LinuxDirent64>>>,
    /// 普通文件共享 inode 上的页缓存；tmpfile 使用独立页缓存。
    page_cache: Option<Arc<PageCache>>,
    /// 每个打开文件描述只报告一次回写错误；dup 和 fork 与其余 `FileInner` 状态共享该游标。
    writeback_error_cursor: Option<WritebackErrorCursor>,
    write_back: bool,
    /// Linux 的 f_ra/FMODE_RANDOM 状态属于打开文件描述，因此 dup/fork 共享该状态，
    /// 独立 open 则从 NORMAL 开始。
    read_ahead_pages: usize,
    /// FMODE_NOREUSE 会阻止后续访问提升缓存页的热度。
    no_reuse: bool,
    tmpfile_meta: Option<TmpFileMeta>,
    atime_override: Option<TimeSpec>,
    mtime_override: Option<TimeSpec>,
    ctime_override: Option<TimeSpec>,
}

/// 文件操作 trait
pub trait FileOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    /// 读取数据到 buf 中，返回读取的字节数，同时更新文件偏移量。
    ///
    /// 成功返回 `n` 时，实现必须已初始化 `buf[..n]`，且 `n <= buf.len()`。
    /// syscall 层可能复用未重复清零的内核中转缓冲，因此不得把未写入的区间计入 `n`。
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize>;
    /// 写入数据到 buf 中，返回写入的字节数，同时更新文件偏移量
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize>;
    /// 在指定位置读取，不读取或修改共享 open-file offset。
    /// 成功返回 `n` 时遵守与 [`FileOp::read`] 相同的 `buf[..n]` 初始化契约。
    fn read_at_offset(&self, _offset: usize, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }
    /// 在指定位置写入，不读取或修改共享 open-file offset。
    fn write_at_offset(&self, _offset: usize, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }
    /// Linux pwrite 路径中，O_APPEND 选择 EOF 作为位置，但不修改共享打开文件偏移；
    /// 其他内核定位写入者忽略 O_APPEND。
    fn pwrite_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        self.write_at_offset(offset, buf)
    }
    /// 检查文件对象是否支持偏移移动。
    fn can_seek(&self) -> SysResult;
    // 移动文件偏移
    fn seek(&self, offset: isize) -> SysResult<usize>;
    // 获得文件偏移
    fn get_offset(&self) -> usize;
    // 获得文件打开标志
    fn get_flags(&self) -> OpenFlags;
    /// 修改共享 open-file status flags。descriptor flags（如 CLOEXEC）不在这里处理。
    fn set_status_flags(&self, _flags: OpenFlags) -> SysResult {
        Err(Errno::EINVAL)
    }
    fn get_stat(&self) -> SysResult<KStat>;
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn mmap_allowed(&self, shared: bool, writable: bool) -> SysResult {
        if !self.readable() {
            return Err(Errno::EACCES);
        }
        if shared && writable {
            // MAP_SHARED|PROT_WRITE 要求打开文件描述可写；
            // MAP_PRIVATE 对只读描述符仍然有效。
            if !self.writable() {
                return Err(Errno::EACCES);
            }
        }
        Ok(())
    }
    fn mmap_zero_filled(&self) -> bool {
        false
    }
    fn mmap_open(&self, _shared: bool, _writable: bool, _pages: usize) {}
    fn mmap_close(&self, _shared: bool, _writable: bool, _pages: usize) {}
    /// 在可写共享映射页的 PTE 变为可写前，为其建立持久后端。
    /// 内存后端文件不会耗尽磁盘块，因此默认实现无需预留空间。
    fn reserve_shared_mmap_write(&self, _offset: usize, _data: &[u8]) -> SysResult {
        Ok(())
    }
    // 非阻塞可读：数据是否立即可用—— pipe 非空 / 文件总是可读
    fn read_ready(&self) -> bool {
        true
    }
    // 非阻塞可写：是否立即可写—— pipe 非满 / 文件总是可写
    fn write_ready(&self) -> bool {
        true
    }
    /// poll/epoll 的异常就绪即使未被请求也需要报告。
    fn poll_hup(&self) -> bool {
        false
    }
    /// 流对端已关闭写半部。与 HUP 不同，只有用户态明确请求 Linux 扩展位时才报告 RDHUP。
    fn poll_rdhup(&self) -> bool {
        false
    }
    fn poll_error(&self) -> bool {
        false
    }
    fn register_poll_waiter(&self, _tid: usize, _events: PollEvents) -> bool {
        false
    }
    fn unregister_poll_waiter(&self, _tid: usize) {}
    fn is_tty(&self) -> bool {
        false
    }
    fn splice_supported(&self) -> bool {
        false
    }
    /// splice 开始消费数据前校验输入侧状态。
    fn validate_splice_read(&self) -> SysResult {
        Ok(())
    }
    /// 将文件缓冲数据刷入存储介质。当前文件系统在内存中，默认无操作。
    fn fsync(&self) -> SysResult<usize> {
        Ok(0)
    }
    fn fdatasync(&self) -> SysResult<usize> {
        self.fsync()
    }
    fn sync_file_range(&self, _offset: usize, _len: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }
    fn filesystem(&self) -> Option<Arc<dyn SuperBlockOp>> {
        None
    }
    /// 调整文件长度。普通文件和 memfd 支持该操作，其他特殊 fd 默认拒绝。
    fn truncate(&self, _size: usize) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }
    /// 将指定范围打洞。默认文件类型不支持，memfd 支持按 Linux seal 语义清零范围。
    fn punch_hole(&self, _offset: usize, _len: usize) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }
    /// 预分配指定范围，并在需要时扩展逻辑大小；无法保证分配成功的后端必须拒绝该操作。
    fn allocate_range(&self, _offset: usize, _len: usize) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }
}

impl File {
    /// 为可写 `MAP_SHARED` 页面在 PTE 开放写权限前建立下层持久空间。
    ///
    /// 文件状态锁把预留与 truncate 串行化；先检查只读挂载、写回能力和文件增长额度，再让
    /// inode materialize 目标 filesystem block。若 ENOSPC/EIO，缺页路径会投递 SIGBUS，
    /// 且共享页仍保持只读，不能把失败延迟到之后的 msync/fsync。
    fn reserve_shared_mmap_write(&self, offset: usize, data: &[u8]) -> SysResult {
        if data.is_empty() {
            return Ok(());
        }
        // 下层分配全过程都持有文件状态锁。truncate 用同一把锁保护下层大小变更与
        // PageCache 缩放，因此页面预留不能在这两个步骤之间重新扩展文件。
        let inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        if inner.path.mnt.is_readonly() {
            return Err(Errno::EROFS);
        }
        if !inner.write_back {
            return Ok(());
        }
        let file_size = inner
            .page_cache
            .as_ref()
            .map(|cache| cache.len())
            .unwrap_or(self.inode.stat(path.as_str())?.size);
        if offset >= file_size {
            return Err(Errno::EIO);
        }
        let reserve_len = data.len().min(file_size - offset);
        let page_idx = offset / PAGE_SIZE;
        let page_offset = offset % PAGE_SIZE;
        let reserve_prefix = page_offset.checked_add(reserve_len).ok_or(Errno::EINVAL)?;
        let page_cache = inner.page_cache.clone().ok_or(Errno::EIO)?;

        // lwext4 没有未写区间预留入口。把页面当前字节写回可实体化稀疏块，
        // 且不改变可见内容或文件大小；若空间不足，会在允许用户态弄脏共享帧前报告 ENOSPC。
        page_cache.reserve_mmap_prefix(page_idx, reserve_prefix, |reserved_prefix| {
            let new_start = reserved_prefix.max(page_offset);
            let additional = reserve_prefix.saturating_sub(new_start);
            check_mount_allocation_available(&inner.path, additional)?;
            let data_start = new_start - page_offset;
            let written = self.inode.write_at(
                path.as_str(),
                offset + data_start,
                &data[data_start..reserve_len],
            )?;
            if written != reserve_len - data_start {
                return Err(Errno::EIO);
            }
            Ok(())
        })?;
        Ok(())
    }

    /// 应用一次 Linux/POSIX 文件访问建议，并更新打开文件描述级别的预读策略。
    ///
    /// NORMAL/RANDOM/SEQUENTIAL/NOREUSE 修改 `FileInner`，因此 dup/fork 共享而独立 open
    /// 不共享。WILLNEED 对范围做尽力同步预取；DONTNEED 先启动范围写回，再仅驱逐被建议范围
    /// 完整覆盖且 clean、未映射、未 pin 的页。预取/回写错误不改变 fadvise 返回值，失败脏页
    /// 保留给正常 fsync 错误游标。
    pub fn fadvise(&self, offset: usize, len: usize, advice: usize) -> SysResult<usize> {
        const POSIX_FADV_NORMAL: usize = 0;
        const POSIX_FADV_RANDOM: usize = 1;
        const POSIX_FADV_SEQUENTIAL: usize = 2;
        const POSIX_FADV_WILLNEED: usize = 3;
        const POSIX_FADV_DONTNEED: usize = 4;
        const POSIX_FADV_NOREUSE: usize = 5;

        let (page_cache, write_back, path, read_ahead_pages) = {
            let mut inner = self.inner.lock();
            match advice {
                POSIX_FADV_NORMAL => {
                    inner.read_ahead_pages = DEFAULT_READ_AHEAD_PAGES;
                    inner.no_reuse = false;
                    return Ok(0);
                }
                POSIX_FADV_RANDOM => {
                    inner.read_ahead_pages = 1;
                    return Ok(0);
                }
                POSIX_FADV_SEQUENTIAL => {
                    inner.read_ahead_pages = SEQUENTIAL_READ_AHEAD_PAGES;
                    return Ok(0);
                }
                POSIX_FADV_NOREUSE => {
                    inner.no_reuse = true;
                    return Ok(0);
                }
                POSIX_FADV_WILLNEED | POSIX_FADV_DONTNEED => {}
                _ => return Err(Errno::EINVAL),
            }
            let visible_path = inner.path.abs_path();
            (
                inner.page_cache.clone(),
                inner.write_back,
                self.storage_path(&visible_path),
                inner.read_ahead_pages,
            )
        };
        let Some(page_cache) = page_cache else {
            return Ok(0);
        };
        let lower = write_back.then_some((&self.inode, path.as_str()));
        if advice == POSIX_FADV_WILLNEED {
            page_cache.prefetch_range(offset, len, lower, read_ahead_pages);
        } else if write_back {
            let end = if len == 0 {
                page_cache.len()
            } else {
                offset.saturating_add(len)
            };
            // Linux 在失效缓存前启动回写，且 fadvise 不返回回写错误。
            // RespOS 没有处理该请求的异步刷写器，因此同步执行同一步骤；
            // 失败页面继续保持脏且固定，由普通错误游标后续报告。
            let _ = page_cache.sync_range(&self.inode, path.as_str(), offset, end);
            page_cache.evict_clean_range(offset, len);
        }
        Ok(0)
    }

    fn storage_path(&self, path: &str) -> alloc::string::String {
        alloc::string::String::from(path)
    }

    /// 将普通文件偏移解析为其缓冲页缓存所使用的同一物理帧。
    /// 没有页缓存的特殊文件返回 None，由 mmap 层走兼容路径。
    pub(crate) fn shared_page_frame(
        &self,
        file_offset: usize,
    ) -> SysResult<Option<(Arc<FrameTracker>, bool)>> {
        let (page_cache, write_back, path, read_ahead_pages, mark_accessed) = {
            let inner = self.inner.lock();
            let visible_path = inner.path.abs_path();
            (
                inner.page_cache.clone(),
                inner.write_back,
                self.storage_path(&visible_path),
                inner.read_ahead_pages,
                !inner.no_reuse,
            )
        };
        let Some(page_cache) = page_cache else {
            return Ok(None);
        };
        let lower = write_back.then_some((&self.inode, path.as_str()));
        let page_index = file_offset / PAGE_SIZE;
        page_cache
            .shared_frame_at(page_index, lower, read_ahead_pages, mark_accessed)
            .map(Some)
    }

    pub fn new(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        let abs_path = path.abs_path();
        let ty = inode.node_type();
        let page_cache = inode.get_page_cache();
        let shared_page_identity = inode.stat(&abs_path).ok().map(|stat| (stat.dev, stat.ino));
        let offset = if flags.contains(OpenFlags::O_APPEND) && ty == InodeType::Regular {
            inode.stat(&abs_path).map(|stat| stat.size).unwrap_or(0)
        } else {
            0
        };
        let write_back = ty == InodeType::Regular && page_cache.is_some();
        if let Some(ref pc) = page_cache {
            let size = inode.stat(&abs_path).map(|stat| stat.size).unwrap_or(0);
            if size > pc.len() {
                pc.resize(size);
            }
        }
        let writeback_error_cursor = page_cache
            .as_ref()
            .map(|page_cache| page_cache.sample_writeback_error());
        Self {
            inode,
            shared_page_identity,
            inner: Mutex::new(FileInner {
                offset,
                path,
                flags,
                dirent_cache: None,
                page_cache,
                writeback_error_cursor,
                write_back,
                read_ahead_pages: DEFAULT_READ_AHEAD_PAGES,
                no_reuse: false,
                tmpfile_meta: None,
                atime_override: None,
                mtime_override: None,
                ctime_override: None,
            }),
        }
    }

    pub fn new_tmpfile(
        path: Arc<Path>,
        inode: Arc<dyn InodeOp>,
        flags: OpenFlags,
        meta: TmpFileMeta,
    ) -> Self {
        let page_cache = Some(PageCache::new(0));
        let writeback_error_cursor = page_cache
            .as_ref()
            .map(|page_cache| page_cache.sample_writeback_error());
        Self {
            inode,
            shared_page_identity: None,
            inner: Mutex::new(FileInner {
                offset: 0,
                path,
                flags,
                dirent_cache: None,
                page_cache,
                writeback_error_cursor,
                write_back: false,
                read_ahead_pages: DEFAULT_READ_AHEAD_PAGES,
                no_reuse: false,
                tmpfile_meta: Some(meta),
                atime_override: None,
                mtime_override: None,
                ctime_override: None,
            }),
        }
    }

    #[track_caller]
    /// 从文件起点读取到 EOF，且不改变调用者共享的 open-file offset。
    ///
    /// 依据当前 stat/PageCache 长度预留内核 Vec，并用显式偏移循环处理短读；文件并发增长时
    /// 可以继续扩展缓冲，缩短时以实际 EOF 结束。该 helper 用于 ELF/shebang 等内核消费者，
    /// 仍须经过普通 read_at 权限、PageCache 一致性与错误传播，不能直接解引用下层存储。
    pub fn read_all(&self) -> SysResult<Vec<u8>> {
        let inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);

        if let Some(ref pc) = inner.page_cache {
            let size = pc.len();
            if size > KERNEL_HEAP_SIZE / 2 {
                let caller = core::panic::Location::caller();
                println!(
                    "read_all rejected oversized file path={} size={} caller={}:{}",
                    visible_path,
                    size,
                    caller.file(),
                    caller.line()
                );
                return Err(Errno::ENOMEM);
            }
            let mut data = Vec::new();
            data.try_reserve_exact(size).map_err(|_| Errno::ENOMEM)?;
            data.resize(size, 0);
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            let n = pc.read_at(0, &mut data, lower)?;
            data.truncate(n);
            return Ok(data);
        }

        let size = self.inode.stat(&path)?.size;
        if size > KERNEL_HEAP_SIZE / 2 {
            let caller = core::panic::Location::caller();
            println!(
                "read_all rejected oversized file path={} size={} caller={}:{}",
                visible_path,
                size,
                caller.file(),
                caller.line()
            );
            return Err(Errno::ENOMEM);
        }
        let mut data = Vec::new();
        data.try_reserve_exact(size).map_err(|_| Errno::ENOMEM)?;
        data.resize(size, 0);
        let mut offset = 0;

        while offset < size {
            let n = self.inode.read_at(&path, offset, &mut data[offset..])?;
            if n == 0 {
                break;
            }
            offset += n;
        }

        data.truncate(offset);
        Ok(data)
    }

    pub fn readdir(&self) -> SysResult<Vec<LinuxDirent64>> {
        Ok(self.readdir_uncached()?.as_ref().clone())
    }

    pub fn readdir_cached(&self, current_off: usize) -> SysResult<Arc<Vec<LinuxDirent64>>> {
        // Linux 的 getdents 会拒绝已被 rmdir 分离但仍打开的目录；
        // 因此必须在使用现有目录流缓存前检查。
        if self
            .inode
            .as_any()
            .downcast_ref::<Ext4Inode>()
            .is_some_and(Ext4Inode::is_unlinked)
        {
            return Err(Errno::ENOENT);
        }
        {
            let inner = self.inner.lock();
            if current_off != 0 {
                if let Some(ref entries) = inner.dirent_cache {
                    return Ok(entries.clone());
                }
            }
        }

        let entries = self.readdir_uncached()?;
        let mut inner = self.inner.lock();
        inner.dirent_cache = Some(entries.clone());
        Ok(entries)
    }

    pub fn clear_dirent_cache(&self) {
        self.inner.lock().dirent_cache = None;
    }

    fn readdir_uncached(&self) -> SysResult<Arc<Vec<LinuxDirent64>>> {
        let path = self.path();
        let mut entries = self.inode.readdir(&path.abs_path())?;
        self.touch_atime_if_needed(&path, InodeType::Directory)?;

        if Arc::ptr_eq(&path.dentry, &path.mnt.root) {
            if let Some(mount) = crate::fs::mount::get_mount_by_vfsmount(&path.mnt) {
                if let Some(parent_ino) = mount
                    .mountpoint
                    .get_parent()
                    .and_then(|parent| parent.get_inode().stat(&parent.abs_path).ok())
                    .map(|stat| stat.ino)
                {
                    for entry in &mut entries {
                        if entry.d_name == b"..\0" {
                            entry.d_ino = parent_ino;
                            break;
                        }
                    }
                }
            }
        }

        Ok(Arc::new(entries))
    }
}

impl File {
    pub fn inode(&self) -> Arc<dyn InodeOp> {
        self.inode.clone()
    }

    pub fn path(&self) -> Arc<Path> {
        self.inner.lock().path.clone()
    }

    pub fn metadata_path(&self) -> alloc::string::String {
        let visible_path = self.inner.lock().path.abs_path();
        self.storage_path(&visible_path)
    }

    pub fn tmpfile_meta(&self) -> Option<TmpFileMeta> {
        self.inner.lock().tmpfile_meta
    }

    fn now_timespec() -> TimeSpec {
        crate::syscall::realtime_timespec()
    }

    pub fn set_times(&self, atime: Option<TimeSpec>, mtime: Option<TimeSpec>) -> SysResult {
        let (path, tmpfile) = {
            let inner = self.inner.lock();
            let visible_path = inner.path.abs_path();
            (
                self.storage_path(&visible_path),
                inner.tmpfile_meta.is_some(),
            )
        };

        if !tmpfile {
            self.inode.set_times(path.as_str(), atime, mtime)?;
        }

        if tmpfile {
            let ctime = Self::now_timespec();
            let mut inner = self.inner.lock();
            if let Some(atime) = atime {
                inner.atime_override = Some(atime);
            }
            if let Some(mtime) = mtime {
                inner.mtime_override = Some(mtime);
            }
            inner.ctime_override = Some(ctime);
        }
        Ok(())
    }

    fn flush_page_cache_if_needed(
        &self,
        pc: &Arc<PageCache>,
        _path: &str,
        force: bool,
    ) -> SysResult<bool> {
        if force || pc.needs_writeback() {
            sync_page_cache_owner(pc)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn update_cached_write_times(inner: &mut FileInner, now: TimeSpec) {
        inner.mtime_override = Some(now);
        inner.ctime_override = Some(now);
    }

    /// 将缓存文件数据和待提交的数据时间戳写入 lwext4，
    /// 但不执行下方 fsync 所使用的文件系统持久化屏障。
    fn sync_cached_data(&self) -> SysResult<bool> {
        let (page_cache, write_back, path) = {
            let inner = self.inner.lock();
            let visible_path = inner.path.abs_path();
            (
                inner.page_cache.clone(),
                inner.write_back,
                self.storage_path(&visible_path),
            )
        };
        let Some(page_cache) = page_cache else {
            return Ok(false);
        };
        if !write_back {
            return Ok(false);
        }
        self.flush_page_cache_if_needed(&page_cache, &path, true)?;
        Ok(true)
    }

    fn check_writeback_error(&self) -> SysResult {
        let mut inner = self.inner.lock();
        let Some(page_cache) = inner.page_cache.clone() else {
            return Ok(());
        };
        let Some(cursor) = inner.writeback_error_cursor.as_mut() else {
            return Ok(());
        };
        page_cache.check_writeback_error(cursor)
    }

    /// 原子协调下层文件长度、PageCache 和所有存活共享映射的 truncate。
    ///
    /// 打开文件状态锁覆盖增长额度检查与提交。缩小时先把边界页范围外的脏数据写回，再改变
    /// 下层长度、裁剪 PageCache/reservation，并使越过新 EOF 的共享 PTE 失效；增长时更新
    /// 逻辑长度，并在跨文件系统块边界但仍处于同一 VM 页时写保护相关共享映射，使下一次
    /// store 经 page-mkwrite 建立后端。
    ///
    /// 任一可失败预处理必须发生在可见长度修改前；成功后更新 ctime/mtime。并发 fsync 由
    /// PageCache writeback exclusion 和 size generation 防止旧写回重新扩展已缩小文件。
    pub fn truncate(&self, size: usize) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        if inner.path.mnt.is_readonly() {
            return Err(Errno::EROFS);
        }
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        let old_size = if let Some(ref pc) = inner.page_cache {
            pc.len()
        } else {
            self.inode.stat(&path)?.size
        };
        check_mount_file_growth(&inner.path, old_size, size)?;
        if let Some(pc) = inner.page_cache.clone() {
            if inner.write_back && size != old_size {
                match self.inode.truncate(&path, size) {
                    Ok(_) => {}
                    Err(Errno::ENOENT) => {}
                    Err(err) => return Err(err),
                }
            }
            pc.resize(size);
            if inner.write_back {
                if size < old_size {
                    self.flush_page_cache_if_needed(&pc, &path, true)?;
                }
                let now = Self::now_timespec();
                Self::update_cached_write_times(&mut inner, now);
            }
        } else {
            self.inode.truncate(&path, size)?;
            if size < old_size {
                if let Some((dev, ino)) = self.shared_page_identity {
                    crate::mm::truncate_shared_file_pages(dev, ino, size);
                }
            }
        }
        if inner.offset > size {
            inner.offset = size;
        }
        let mapping_identity = self.shared_page_identity;
        let block_size = mount_block_size(&inner.path);
        drop(inner);
        if let Some((dev, ino)) = mapping_identity {
            if size < old_size {
                crate::mm::truncate_file_mappings(dev, ino, size);
            } else if size > old_size {
                crate::mm::protect_extended_file_mappings(dev, ino, old_size, size, block_size);
            }
        }
        Ok(0)
    }

    /// 在保持文件逻辑长度不变的前提下，把指定范围变为稀疏 hole。
    ///
    /// 文件状态锁与 PageCache writeback exclusion 串行化 truncate/fsync。边界部分页先写回范围外
    /// 脏字节，再由下层清零/释放 extent；随后清零或失效 PageCache 页面并同步所有 MAP_SHARED
    /// 帧。任一预处理失败不得开始破坏性下层操作，成功后更新 mtime/ctime 与写回代次。
    pub fn punch_hole(&self, offset: usize, len: usize) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        if inner.path.mnt.is_readonly() {
            return Err(Errno::EROFS);
        }
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        let file_size = inner
            .page_cache
            .as_ref()
            .map(|cache| cache.len())
            .unwrap_or(self.inode.stat(&path)?.size);
        let end = offset.saturating_add(len).min(file_size);
        if offset >= end {
            return Ok(0);
        }

        if let Some(page_cache) = inner.page_cache.clone() {
            if !inner.write_back {
                return Err(Errno::EOPNOTSUPP);
            }
            // 这里刻意使用页粒度：原地清零部分边界块之前，该块范围外的脏字节
            // 必须先到达下层存储。
            page_cache.sync_range(&self.inode, path.as_str(), offset, end)?;
            // 破坏性区间事务前先发布较早延迟的 mtime/ctime；否则后续元数据刷新可能重载并
            // 覆盖 inode 刚刚减小的 i_blocks 值。
            self.inode.flush_data_metadata(path.as_str())?;
            page_cache.with_writeback_exclusion(|| {
                self.inode.punch_hole(path.as_str(), offset, end - offset)?;
                page_cache.punch_hole(offset, end - offset);
                Ok(())
            })?;
            let now = Self::now_timespec();
            Self::update_cached_write_times(&mut inner, now);
        } else {
            self.inode.punch_hole(path.as_str(), offset, end - offset)?;
            if let Some((dev, ino)) = self.shared_page_identity {
                crate::mm::punch_shared_file_pages(dev, ino, offset, end - offset);
            }
        }
        let identity = self.shared_page_identity;
        drop(inner);
        if let Some((dev, ino)) = identity {
            crate::mm::punch_file_mappings(dev, ino, offset, end - offset);
        }
        Ok(0)
    }

    pub fn read_at_offset(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
        let (path, file_path, page_cache, write_back, read_ahead_pages, mark_accessed) = {
            let inner = self.inner.lock();
            let visible_path = inner.path.abs_path();
            (
                self.storage_path(&visible_path),
                inner.path.clone(),
                inner.page_cache.clone(),
                inner.write_back,
                inner.read_ahead_pages,
                !inner.no_reuse,
            )
        };
        let ty = self.inode.node_type();
        let n = if let Some(ref pc) = page_cache {
            let lower = write_back.then_some((&self.inode, path.as_str()));
            pc.read_at_advised(offset, buf, lower, read_ahead_pages, mark_accessed)
        } else {
            self.inode.read_at(&path, offset, buf)
        }?;
        if page_cache.is_none() && n != 0 {
            if let Some((dev, ino)) = self.shared_page_identity {
                crate::mm::overlay_shared_file_pages(dev, ino, offset, &mut buf[..n]);
            }
        }
        self.touch_atime_if_needed(&file_path, ty)?;
        Ok(n)
    }

    /// 在显式偏移处写入，不读取也不修改打开文件描述的共享游标。
    ///
    /// 写入前检查只读挂载、偏移溢出和文件系统增长额度。具有 PageCache 的普通文件先更新
    /// 共享缓存页和逻辑长度，再登记 dirty owner 与 mtime/ctime；`O_SYNC`/`O_DSYNC` 会强制
    /// 当前缓存及文件系统屏障。无 PageCache 后端直接写 inode，并同步覆盖仍存活的共享文件映射。
    ///
    /// 本入口不实现 `O_APPEND` 位置选择；需要 Linux pwrite+append 语义的调用者应在持有
    /// `FileInner` 锁的路径中原子选择 EOF，避免两个写者取得同一旧长度。
    pub fn write_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        let (path, file_path, page_cache, write_back, flags) = {
            let inner = self.inner.lock();
            let visible_path = inner.path.abs_path();
            (
                self.storage_path(&visible_path),
                inner.path.clone(),
                inner.page_cache.clone(),
                inner.write_back,
                inner.flags,
            )
        };
        if !buf.is_empty() && file_path.mnt.is_readonly() {
            return Err(Errno::EROFS);
        }
        let old_size = if let Some(ref pc) = page_cache {
            pc.len()
        } else {
            self.inode.stat(&path)?.size
        };
        if !buf.is_empty() {
            let requested_end = offset.checked_add(buf.len()).ok_or(Errno::EINVAL)?;
            check_mount_file_growth(&file_path, old_size, requested_end)?;
        }
        if let Some(pc) = page_cache {
            let lower = write_back.then_some((&self.inode, path.as_str()));
            let n = pc.write_at(offset, buf, lower)?;
            let end = offset.checked_add(n).ok_or(Errno::EINVAL)?;
            if end > pc.len() {
                pc.resize(end);
            }
            if write_back && n != 0 {
                let force = flags.intersects(OpenFlags::O_DSYNC | OpenFlags::O_SYNC);
                let now = Self::now_timespec();
                {
                    let mut inner = self.inner.lock();
                    Self::update_cached_write_times(&mut inner, now);
                }
                let time_result = self.inode.note_data_write(path.as_str(), now);
                register_dirty_owner(
                    pc.clone(),
                    self.inode.clone(),
                    file_path.mnt.fs.clone(),
                    path.as_str(),
                );
                time_result?;
                self.flush_page_cache_if_needed(&pc, &path, force)?;
                if force {
                    file_path.mnt.fs.sync()?;
                }
            }
            Ok(n)
        } else {
            let n = self.inode.write_at(&path, offset, buf)?;
            if n != 0 {
                if let Some((dev, ino)) = self.shared_page_identity {
                    crate::mm::update_shared_file_pages(dev, ino, offset, &buf[..n]);
                }
            }
            Ok(n)
        }
    }

    /// 在已持有打开文件描述状态锁时执行写入，是共享 offset 与 `O_APPEND` 的提交内核。
    ///
    /// 锁覆盖 EOF/offset 选择、PageCache 写入和必要的同步刷新，使 dup/fork 共享的同一
    /// open-file description 不会交错更新游标。不得在没有该锁的路径中复刻 append 逻辑；
    /// 显式定位且忽略 append 的内核写入应使用 `write_at_offset`。
    fn write_locked(&self, inner: &mut FileInner, offset: usize, buf: &[u8]) -> SysResult<usize> {
        if !buf.is_empty() && inner.path.mnt.is_readonly() {
            return Err(Errno::EROFS);
        }
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        let old_size = if let Some(ref pc) = inner.page_cache {
            pc.len()
        } else {
            self.inode.stat(&path)?.size
        };
        if !buf.is_empty() {
            let requested_end = offset.checked_add(buf.len()).ok_or(Errno::EINVAL)?;
            check_mount_file_growth(&inner.path, old_size, requested_end)?;
        }
        if let Some(pc) = inner.page_cache.clone() {
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            let n = pc.write_at(offset, buf, lower)?;
            let end = offset.checked_add(n).ok_or(Errno::EINVAL)?;
            if end > pc.len() {
                pc.resize(end);
            }
            if inner.write_back && n != 0 {
                let force = inner
                    .flags
                    .intersects(OpenFlags::O_DSYNC | OpenFlags::O_SYNC);
                let now = Self::now_timespec();
                Self::update_cached_write_times(inner, now);
                let time_result = self.inode.note_data_write(path.as_str(), now);
                register_dirty_owner(
                    pc.clone(),
                    self.inode.clone(),
                    inner.path.mnt.fs.clone(),
                    path.as_str(),
                );
                time_result?;
                self.flush_page_cache_if_needed(&pc, &path, force)?;
                if force {
                    inner.path.mnt.fs.sync()?;
                }
            }
            Ok(n)
        } else {
            let n = self.inode.write_at(&path, offset, buf)?;
            if n != 0 {
                if let Some((dev, ino)) = self.shared_page_identity {
                    crate::mm::update_shared_file_pages(dev, ino, offset, &buf[..n]);
                }
            }
            Ok(n)
        }
    }

    pub fn pwrite_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let write_offset = if inner.flags.contains(OpenFlags::O_APPEND) {
            let visible_path = inner.path.abs_path();
            let path = self.storage_path(&visible_path);
            if let Some(ref pc) = inner.page_cache {
                pc.len()
            } else {
                self.inode.stat(&path)?.size
            }
        } else {
            offset
        };
        self.write_locked(&mut inner, write_offset, buf)
    }

    /// 按 mount 的 noatime/nodiratime/relatime/strictatime/lazytime 策略提交一次访问时间更新。
    ///
    /// 先依据 inode 类型和当前时间判断是否需要更新；lazytime 只登记内存态 pending atime，
    /// strict/relatime 需要时写入下层。时间更新失败在已有读取进展后由上层按 partial-I/O 规则
    /// 处理，不能让缓存锁与 ext4 全局锁形成反向锁序。
    fn touch_atime_if_needed(
        &self,
        path: &Arc<Path>,
        ty: InodeType,
    ) -> SysResult<Option<TimeSpec>> {
        if path.mnt.is_readonly()
            || path.mnt.has_flag(MS_NOATIME)
            || (ty == InodeType::Directory && path.mnt.has_flag(MS_NODIRATIME))
        {
            return Ok(None);
        }

        let (flags, tmpfile, cached_atime, cached_mtime, cached_ctime) = {
            let inner = self.inner.lock();
            (
                inner.flags,
                inner.tmpfile_meta.is_some(),
                inner.atime_override,
                inner.mtime_override,
                inner.ctime_override,
            )
        };
        if flags.contains(OpenFlags::O_NOATIME) {
            return Ok(None);
        }

        let now = Self::now_timespec();
        let visible_path = path.abs_path();
        let storage_path = self.storage_path(&visible_path);
        let (atime, mtime, ctime) = if tmpfile {
            (
                cached_atime.unwrap_or(now),
                cached_mtime.unwrap_or(now),
                cached_ctime.unwrap_or(now),
            )
        } else {
            let stat = self.inode.stat(storage_path.as_str())?;
            (stat.atime, stat.mtime, stat.ctime)
        };

        let timestamp_le = |left: TimeSpec, right: TimeSpec| {
            left.sec < right.sec || (left.sec == right.sec && left.nsec <= right.nsec)
        };
        let stale = now.sec.saturating_sub(atime.sec) >= 24 * 60 * 60;
        if !path.mnt.has_flag(MS_STRICTATIME)
            && !timestamp_le(atime, mtime)
            && !timestamp_le(atime, ctime)
            && !stale
        {
            return Ok(None);
        }

        if !tmpfile {
            if let Some(inode) = self.inode.as_any().downcast_ref::<Ext4Inode>() {
                if path.mnt.has_flag(MS_LAZYTIME) {
                    inode.defer_atime(storage_path.as_str(), now)?;
                    crate::fs::ext4::register_lazytime_inode(self.inode.clone());
                } else {
                    inode.touch_atime(storage_path.as_str(), now)?;
                }
            }
        }
        Ok(Some(now))
    }
}

impl Drop for File {
    fn drop(&mut self) {
        crate::perf::file_close(1);
    }
}

impl FileOp for File {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn reserve_shared_mmap_write(&self, offset: usize, data: &[u8]) -> SysResult {
        File::reserve_shared_mmap_write(self, offset, data)
    }

    fn mmap_zero_filled(&self) -> bool {
        self.inode.mmap_zero_filled()
    }

    fn splice_supported(&self) -> bool {
        true
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        let offset = inner.offset;
        let file_path = inner.path.clone();
        let ty = self.inode.node_type();
        let n = if let Some(ref pc) = inner.page_cache {
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            pc.read_at_advised(offset, buf, lower, inner.read_ahead_pages, !inner.no_reuse)?
        } else {
            self.inode.read_at(&path, offset, buf)?
        };
        if inner.page_cache.is_none() && n != 0 {
            if let Some((dev, ino)) = self.shared_page_identity {
                crate::mm::overlay_shared_file_pages(dev, ino, offset, &mut buf[..n]);
            }
        }
        inner.offset += n;
        drop(inner);
        self.touch_atime_if_needed(&file_path, ty)?;
        Ok(n)
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        if inner.flags.contains(OpenFlags::O_APPEND) {
            let append_off = if let Some(ref pc) = inner.page_cache {
                pc.len()
            } else {
                self.inode.stat(&path)?.size
            };
            inner.offset = append_off;
        }

        let offset = inner.offset;
        let n = self.write_locked(&mut inner, offset, buf)?;
        inner.offset += n;
        Ok(n)
    }

    fn read_at_offset(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
        File::read_at_offset(self, offset, buf)
    }

    fn write_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        File::write_at_offset(self, offset, buf)
    }

    fn pwrite_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        File::pwrite_at_offset(self, offset, buf)
    }

    fn seek(&self, offset: isize) -> SysResult<usize> {
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        let mut inner = self.inner.lock();
        inner.offset = offset;
        if offset == 0 {
            inner.dirent_cache = None;
        }
        Ok(offset)
    }

    fn can_seek(&self) -> SysResult {
        if self.get_flags().contains(OpenFlags::O_PATH) {
            return Err(Errno::EBADF);
        }
        let ty = self.get_stat()?.ty;
        if ty == InodeType::Regular || ty == InodeType::Directory {
            Ok(())
        } else {
            Err(Errno::ESPIPE)
        }
    }

    fn get_offset(&self) -> usize {
        self.inner.lock().offset
    }

    fn get_flags(&self) -> OpenFlags {
        self.inner.lock().flags
    }

    fn set_status_flags(&self, flags: OpenFlags) -> SysResult {
        let status_flags = OpenFlags::O_APPEND | OpenFlags::O_NONBLOCK | OpenFlags::O_DIRECT;
        let mut inner = self.inner.lock();
        inner.flags.remove(status_flags);
        inner.flags |= flags & status_flags;
        Ok(())
    }

    fn get_stat(&self) -> SysResult<KStat> {
        let inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        if let Some(ref pc) = inner.page_cache {
            let mut stat = self.inode.stat(&path)?;
            stat.size = pc.len();
            if let Some(meta) = inner.tmpfile_meta {
                stat.blocks = KStat::blocks_for_size(stat.size as u64);
                stat.ty = InodeType::Regular;
                stat.mode = meta.mode;
                stat.uid = meta.uid;
                stat.gid = meta.gid;
                stat.nlink = 0;
                if let Some(atime) = inner.atime_override {
                    stat.atime = atime;
                }
                if let Some(mtime) = inner.mtime_override {
                    stat.mtime = mtime;
                }
                if let Some(ctime) = inner.ctime_override {
                    stat.ctime = ctime;
                }
            }
            return Ok(stat);
        }
        let mut stat = self.inode.stat(&path)?;
        if let Some(atime) = inner.atime_override {
            stat.atime = atime;
        }
        if let Some(mtime) = inner.mtime_override {
            stat.mtime = mtime;
        }
        if let Some(ctime) = inner.ctime_override {
            stat.ctime = ctime;
        }
        Ok(stat)
    }

    fn readable(&self) -> bool {
        let flags = self.get_flags();
        !flags.contains(OpenFlags::O_PATH) && !flags.contains(OpenFlags::O_WRONLY)
    }

    fn read_ready(&self) -> bool {
        self.inode.read_ready()
    }

    fn writable(&self) -> bool {
        let flags = self.get_flags();
        !flags.contains(OpenFlags::O_PATH)
            && flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    }

    fn is_tty(&self) -> bool {
        self.get_stat()
            .map(|stat| stat.ty == InodeType::CharDevice && stat.rdev >> 8 == 5)
            .unwrap_or(false)
    }

    fn fsync(&self) -> SysResult<usize> {
        crate::perf::explicit_fsync(1);
        let (superblock, path) = {
            let inner = self.inner.lock();
            let visible_path = inner.path.abs_path();
            (inner.path.mnt.fs.clone(), self.storage_path(&visible_path))
        };
        let data_result = self.sync_cached_data();
        // 同时消费本次尝试和更早的 close/阈值回写所记录的错误，
        // 避免同一打开文件描述把一次失败报告两遍。
        let writeback_error = self.check_writeback_error();
        data_result?;
        writeback_error?;
        self.inode.flush_data_metadata(path.as_str())?;
        superblock.sync()?;
        Ok(0)
    }

    fn fdatasync(&self) -> SysResult<usize> {
        // lwext4 只暴露一种文件系统持久化屏障。这里使用它虽强于 fdatasync 的最低契约，
        // 但能保证数据与大小元数据的必要顺序，而无需伪造较弱路径。
        self.fsync()
    }

    fn sync_file_range(&self, offset: usize, len: usize) -> SysResult<usize> {
        let page_cache = self.inner.lock().page_cache.clone().ok_or(Errno::EINVAL)?;
        let end = if len == 0 {
            usize::MAX
        } else {
            offset.checked_add(len).ok_or(Errno::EINVAL)?
        };
        sync_page_cache_range(&page_cache, offset, end)?;
        self.check_writeback_error()?;
        Ok(0)
    }

    fn filesystem(&self) -> Option<Arc<dyn SuperBlockOp>> {
        Some(self.inner.lock().path.mnt.fs.clone())
    }

    fn truncate(&self, size: usize) -> SysResult<usize> {
        File::truncate(self, size)
    }

    fn punch_hole(&self, offset: usize, len: usize) -> SysResult<usize> {
        File::punch_hole(self, offset, len)
    }
}

bitflags::bitflags! {
    pub struct OpenFlags: u32 {
        const O_RDONLY = 0;
        const O_WRONLY = 1 << 0;
        const O_RDWR   = 1 << 1;
        const O_CREATE = 1 << 6;
        const O_EXCL   = 1 << 7;
        const O_NOCTTY = 1 << 8;
        const O_TRUNC  = 1 << 9;
        const O_NONBLOCK = 1 << 11;
        const O_DSYNC = 1 << 12;
        const O_DIRECT = 1 << 14;
        const O_APPEND = 1 << 10;
        const O_DIRECTORY = 1 << 16;
        const O_NOFOLLOW = 1 << 17;
        const O_CLOEXEC = 1 << 19;
        const O_SYNC = (1 << 20) | Self::O_DSYNC.bits();
        const O_NOATIME = 1 << 18;
        const O_PATH = 0o10000000;
        const O_TMPFILE = 0x410000;
    }
}

impl From<usize> for OpenFlags {
    fn from(bits: usize) -> Self {
        Self::from_bits_truncate(bits as u32)
    }
}
impl From<OpenFlags> for usize {
    fn from(flags: OpenFlags) -> Self {
        flags.bits() as usize
    }
}
