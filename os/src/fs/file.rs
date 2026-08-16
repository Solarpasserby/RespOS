// os/src/vfs/file.rs

use super::vfs::{InodeOp, InodeType, LinuxDirent64, SuperBlockOp};
use crate::config::{KERNEL_HEAP_SIZE, PAGE_SIZE};
use crate::fs::ext4::Ext4Inode;
use crate::fs::mount::{
    MS_LAZYTIME, MS_NOATIME, MS_NODIRATIME, MS_STRICTATIME, check_mount_file_growth,
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

// 常规文件
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
    /// Cached directory entries for the current directory-stream pass.
    /// Rebuilt whenever getdents starts from offset 0.
    dirent_cache: Option<Arc<Vec<LinuxDirent64>>>,
    /// 普通文件共享 inode 上的页缓存；tmpfile 使用独立页缓存。
    page_cache: Option<Arc<PageCache>>,
    /// Writeback errors are reported once per open-file description. dup and
    /// fork share this cursor together with the rest of `FileInner`.
    writeback_error_cursor: Option<WritebackErrorCursor>,
    write_back: bool,
    /// Linux f_ra/FMODE_RANDOM state belongs to the open-file description,
    /// so dup/fork share it while an independent open starts at NORMAL.
    read_ahead_pages: usize,
    /// FMODE_NOREUSE suppresses cache promotion on subsequent accesses.
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
    /// Linux pwrite path: O_APPEND selects EOF without modifying the shared
    /// open-file offset. Other positioned kernel writers ignore O_APPEND.
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
            // MAP_SHARED|PROT_WRITE needs a writable open-file description;
            // MAP_PRIVATE remains valid on a read-only descriptor.
            if !self.writable() {
                return Err(Errno::EACCES);
            }
        }
        Ok(())
    }
    fn mmap_open(&self, _shared: bool, _writable: bool, _pages: usize) {}
    fn mmap_close(&self, _shared: bool, _writable: bool, _pages: usize) {}
    /// Establish persistent backing for a writable shared-mapping page before
    /// its PTE becomes writable. Memory-backed files cannot run out of disk
    /// blocks, so their default implementation needs no reservation.
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
    /// poll/epoll exceptional readiness is reported even when not requested.
    fn poll_hup(&self) -> bool {
        false
    }
    /// Stream peer closed its write half. Unlike HUP, RDHUP is reported only
    /// when userspace explicitly requests the Linux extension bit.
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
    /// Validate input-side state before splice starts consuming data.
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
    /// Preallocate the range and extend the logical size when needed.
    /// Backends that cannot honor allocation guarantees must reject it.
    fn allocate_range(&self, _offset: usize, _len: usize) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }
}

impl File {
    fn reserve_shared_mmap_write(&self, offset: usize, data: &[u8]) -> SysResult {
        if data.is_empty() {
            return Ok(());
        }
        // Keep the file state lock through the lower allocation. truncate
        // uses the same lock for lower size change and PageCache resize, so a
        // page reservation cannot re-extend a file between those two steps.
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

        // lwext4 has no unwritten-extent reservation entry point. Writing the
        // page's current bytes materializes a sparse block without changing
        // visible contents or file size, and reports ENOSPC before userspace
        // is allowed to dirty the shared frame.
        let written = self
            .inode
            .write_at(path.as_str(), offset, &data[..reserve_len])?;
        if written != reserve_len {
            return Err(Errno::EIO);
        }
        Ok(())
    }

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
            // Linux starts writeback before invalidation and does not return
            // writeback errors from fadvise.  RespOS has no async flusher for
            // this request, so perform the same step synchronously and leave
            // failed pages dirty/pinned for the normal error cursor.
            let _ = page_cache.sync_range(&self.inode, path.as_str(), offset, end);
            page_cache.evict_clean_range(offset, len);
        }
        Ok(0)
    }

    fn storage_path(&self, path: &str) -> alloc::string::String {
        alloc::string::String::from(path)
    }

    /// Resolve a regular-file offset to the same physical frame used by its
    /// buffered page cache.  Special files without a page cache return None
    /// and let the mmap layer use its compatibility path.
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
        // Linux getdents rejects an open directory after it has been detached
        // by rmdir. Check before serving an existing directory-stream cache.
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

    /// Write cached file data and pending data timestamps to lwext4 without
    /// issuing the filesystem durability barrier performed by fsync below.
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
        let truncated_identity = (size < old_size)
            .then_some(self.shared_page_identity)
            .flatten();
        drop(inner);
        if let Some((dev, ino)) = truncated_identity {
            crate::mm::truncate_file_mappings(dev, ino, size);
        }
        Ok(0)
    }

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
            // Page granularity is deliberate: dirty bytes outside a partial
            // boundary block must reach lower storage before that block is
            // zeroed in place.
            page_cache.sync_range(&self.inode, path.as_str(), offset, end)?;
            // Publish older delayed mtime/ctime before the destructive extent
            // transaction. Otherwise a later metadata flush can reload and
            // overwrite the inode's newly reduced i_blocks value.
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
        // Consume an error recorded by this attempt as well as an earlier
        // close/threshold writeback. This prevents a single failure from
        // being reported twice by the same open-file description.
        let writeback_error = self.check_writeback_error();
        data_result?;
        writeback_error?;
        self.inode.flush_data_metadata(path.as_str())?;
        superblock.sync()?;
        Ok(0)
    }

    fn fdatasync(&self) -> SysResult<usize> {
        // lwext4 exposes one filesystem durability barrier.  Using it here is
        // stronger than the minimum fdatasync contract but preserves the
        // required data/size metadata ordering without a fake weaker path.
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
