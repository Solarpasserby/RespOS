// os/src/vfs/file.rs

use super::vfs::{InodeOp, InodeType, LinuxDirent64};
use crate::fs::ext4::Ext4Inode;
use crate::fs::mount::{MS_NOATIME, MS_NODIRATIME, MS_STRICTATIME, check_mount_file_growth};
use crate::fs::page_cache::PageCache;
use crate::fs::{KStat, Path, PollEvents};
use crate::syscall::{Errno, SysResult};
use crate::timer::{TimeSpec, get_time_ms};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

// 常规文件
pub struct File {
    inode: Arc<dyn InodeOp>,
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
    write_back: bool,
    tmpfile_meta: Option<TmpFileMeta>,
    atime_override: Option<TimeSpec>,
    mtime_override: Option<TimeSpec>,
    ctime_override: Option<TimeSpec>,
}

/// 文件操作 trait
pub trait FileOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    /// 读取数据到 buf 中，返回读取的字节数，同时更新文件偏移量
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize>;
    /// 写入数据到 buf 中，返回写入的字节数，同时更新文件偏移量
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize>;
    /// 检查文件对象是否支持偏移移动。
    fn can_seek(&self) -> SysResult;
    // 移动文件偏移
    fn seek(&self, offset: isize) -> SysResult<usize>;
    // 获得文件偏移
    fn get_offset(&self) -> usize;
    // 获得文件打开标志
    fn get_flags(&self) -> OpenFlags;
    fn get_stat(&self) -> SysResult<KStat>;
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn mmap_allowed(&self, shared: bool, writable: bool) -> SysResult {
        if !self.readable() {
            return Err(Errno::EACCES);
        }
        if shared && writable && !self.writable() {
            return Err(Errno::EACCES);
        }
        Ok(())
    }
    fn mmap_open(&self, _shared: bool, _writable: bool, _pages: usize) {}
    fn mmap_close(&self, _shared: bool, _writable: bool, _pages: usize) {}
    // 非阻塞可读：数据是否立即可用—— pipe 非空 / 文件总是可读
    fn read_ready(&self) -> bool {
        true
    }
    // 非阻塞可写：是否立即可写—— pipe 非满 / 文件总是可写
    fn write_ready(&self) -> bool {
        true
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
    /// 将文件缓冲数据刷入存储介质。当前文件系统在内存中，默认无操作。
    fn fsync(&self) -> SysResult<usize> {
        Ok(0)
    }
    /// 调整文件长度。普通文件和 memfd 支持该操作，其他特殊 fd 默认拒绝。
    fn truncate(&self, _size: usize) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }
    /// 将指定范围打洞。默认文件类型不支持，memfd 支持按 Linux seal 语义清零范围。
    fn punch_hole(&self, _offset: usize, _len: usize) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }
}

impl File {
    fn storage_path(&self, path: &str) -> alloc::string::String {
        self.inode
            .as_any()
            .downcast_ref::<Ext4Inode>()
            .map(|inode| inode.storage_path(path))
            .unwrap_or_else(|| alloc::string::String::from(path))
    }

    pub fn new(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        let abs_path = path.abs_path();
        let ty = inode.node_type();
        let page_cache = inode.get_page_cache();
        if flags.contains(OpenFlags::O_TRUNC)
            && flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
        {
            let _ = inode.truncate(&abs_path, 0);
            if let Some(ref pc) = page_cache {
                pc.resize(0);
            }
        }
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
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset,
                path,
                flags,
                dirent_cache: None,
                page_cache,
                write_back,
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
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset: 0,
                path,
                flags,
                dirent_cache: None,
                page_cache,
                write_back: false,
                tmpfile_meta: Some(meta),
                atime_override: None,
                mtime_override: None,
                ctime_override: None,
            }),
        }
    }

    pub fn read_all(&self) -> SysResult<Vec<u8>> {
        let inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);

        if let Some(ref pc) = inner.page_cache {
            let size = pc.len();
            let mut data = Vec::new();
            data.try_reserve_exact(size).map_err(|_| Errno::ENOMEM)?;
            data.resize(size, 0);
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            let n = pc.read_at(0, &mut data, lower)?;
            data.truncate(n);
            return Ok(data);
        }

        let size = self.inode.stat(&path)?.size;
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
        if let Some(atime) = self.touch_atime_if_needed(&path, InodeType::Directory)? {
            self.inner.lock().atime_override = Some(atime);
        }

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

    pub fn tmpfile_meta(&self) -> Option<TmpFileMeta> {
        self.inner.lock().tmpfile_meta
    }

    fn now_timespec() -> TimeSpec {
        let ms = get_time_ms();
        TimeSpec {
            sec: (ms / 1000) as isize,
            nsec: ((ms % 1000) * 1_000_000) as isize,
        }
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
            match self.inode.set_times(path.as_str(), atime, mtime) {
                Ok(_) | Err(Errno::ENOENT) => {}
                Err(err) => return Err(err),
            }
        }

        let ctime = Self::now_timespec();
        let mut inner = self.inner.lock();
        if let Some(atime) = atime {
            inner.atime_override = Some(atime);
        }
        if let Some(mtime) = mtime {
            inner.mtime_override = Some(mtime);
        }
        inner.ctime_override = Some(ctime);
        Ok(())
    }

    fn flush_page_cache_if_needed(
        &self,
        pc: &Arc<PageCache>,
        path: &str,
        force: bool,
    ) -> SysResult<bool> {
        if force || pc.needs_writeback() {
            match pc.sync(&self.inode, path) {
                Ok(_) | Err(Errno::ENOENT) => Ok(true),
                Err(err) => Err(err),
            }
        } else {
            Ok(false)
        }
    }

    fn update_cached_write_times(inner: &mut FileInner, now: TimeSpec) {
        inner.mtime_override = Some(now);
        inner.ctime_override = Some(now);
    }

    fn sync_cached_write_times(&self, inner: &FileInner, path: &str) -> SysResult {
        if inner.tmpfile_meta.is_none() {
            if let Some(mtime) = inner.mtime_override {
                self.inode.set_times(path, None, Some(mtime))?;
            }
        }
        Ok(())
    }

    pub fn truncate(&self, size: usize) -> SysResult<usize> {
        let mut inner = self.inner.lock();
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
                self.sync_cached_write_times(&inner, &path)?;
            }
        } else {
            self.inode.truncate(&path, size)?;
        }
        if inner.offset > size {
            drop(inner);
            self.inner.lock().offset = size;
        }
        Ok(0)
    }

    pub fn read_at_offset(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
        let inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        let file_path = inner.path.clone();
        let ty = self.inode.node_type();
        let n = if let Some(ref pc) = inner.page_cache {
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            pc.read_at(offset, buf, lower)
        } else {
            self.inode.read_at(&path, offset, buf)
        }?;
        drop(inner);
        if let Some(atime) = self.touch_atime_if_needed(&file_path, ty)? {
            self.inner.lock().atime_override = Some(atime);
        }
        Ok(n)
    }

    pub fn write_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
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
                Self::update_cached_write_times(&mut inner, now);
                if self.flush_page_cache_if_needed(&pc, &path, force)? {
                    self.sync_cached_write_times(&inner, &path)?;
                }
            }
            Ok(n)
        } else {
            self.inode.write_at(&path, offset, buf)
        }
    }

    fn touch_atime_if_needed(
        &self,
        path: &Arc<Path>,
        ty: InodeType,
    ) -> SysResult<Option<TimeSpec>> {
        if path.mnt.has_flag(MS_NOATIME)
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
            (
                cached_atime.unwrap_or(stat.atime),
                cached_mtime.unwrap_or(stat.mtime),
                cached_ctime.unwrap_or(stat.ctime),
            )
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
            if self.inode.as_any().downcast_ref::<Ext4Inode>().is_some() {
                self.inode
                    .set_times(storage_path.as_str(), Some(now), None)?;
            }
        }
        Ok(Some(now))
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = <Self as FileOp>::fsync(self);
        if let Some(inode) = self.inode.as_any().downcast_ref::<Ext4Inode>() {
            inode.cleanup_orphan();
        }
    }
}

impl FileOp for File {
    fn as_any(&self) -> &dyn Any {
        self
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
            pc.read_at(offset, buf, lower)?
        } else {
            self.inode.read_at(&path, offset, buf)?
        };
        inner.offset += n;
        drop(inner);
        if let Some(atime) = self.touch_atime_if_needed(&file_path, ty)? {
            self.inner.lock().atime_override = Some(atime);
        }
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
        let old_size = if let Some(ref pc) = inner.page_cache {
            pc.len()
        } else {
            self.inode.stat(&path)?.size
        };
        if !buf.is_empty() {
            let requested_end = offset.checked_add(buf.len()).ok_or(Errno::EINVAL)?;
            check_mount_file_growth(&inner.path, old_size, requested_end)?;
        }
        let n = if let Some(pc) = inner.page_cache.clone() {
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
                Self::update_cached_write_times(&mut inner, now);
                if self.flush_page_cache_if_needed(&pc, &path, force)? {
                    self.sync_cached_write_times(&inner, &path)?;
                }
            }
            n
        } else {
            self.inode.write_at(&path, offset, buf)?
        };
        inner.offset += n;
        Ok(n)
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

    fn get_stat(&self) -> SysResult<KStat> {
        let inner = self.inner.lock();
        let visible_path = inner.path.abs_path();
        let path = self.storage_path(&visible_path);
        if let Some(ref pc) = inner.page_cache {
            let mut stat = match self.inode.stat(&path) {
                Ok(stat) => stat,
                Err(Errno::ENOENT) => KStat::minimal(0, InodeType::Regular),
                Err(err) => return Err(err),
            };
            stat.size = pc.len();
            stat.blocks = KStat::blocks_for_size(stat.size as u64);
            if let Some(meta) = inner.tmpfile_meta {
                stat.ty = InodeType::Regular;
                stat.mode = meta.mode;
                stat.uid = meta.uid;
                stat.gid = meta.gid;
                stat.nlink = 0;
            }
            if let Some(atime) = inner.atime_override {
                stat.atime = atime;
            }
            if let Some(mtime) = inner.mtime_override {
                stat.mtime = mtime;
            }
            if let Some(ctime) = inner.ctime_override {
                stat.ctime = ctime;
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
        !self.get_flags().contains(OpenFlags::O_WRONLY)
    }

    fn writable(&self) -> bool {
        self.get_flags()
            .intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    }

    fn is_tty(&self) -> bool {
        self.get_stat()
            .map(|stat| stat.ty == InodeType::CharDevice && stat.rdev >> 8 == 5)
            .unwrap_or(false)
    }

    fn fsync(&self) -> SysResult<usize> {
        let inner = self.inner.lock();
        if let Some(ref pc) = inner.page_cache {
            if inner.write_back {
                let visible_path = inner.path.abs_path();
                let path = self.storage_path(&visible_path);
                self.flush_page_cache_if_needed(pc, &path, true)?;
                self.sync_cached_write_times(&inner, &path)?;
            }
        }
        Ok(0)
    }

    fn truncate(&self, size: usize) -> SysResult<usize> {
        File::truncate(self, size)
    }
}

bitflags::bitflags! {
    pub struct OpenFlags: u32 {
        const O_RDONLY = 0;
        const O_WRONLY = 1 << 0;
        const O_RDWR   = 1 << 1;
        const O_CREATE = 1 << 6;
        const O_EXCL   = 1 << 7;
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
