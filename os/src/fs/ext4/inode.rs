// os/src/ext4/inode.rs

use crate::config::INODE_CACHE_CAPACITY;
use alloc::ffi::CString;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};
use hashbrown::HashMap;
use lazy_static::lazy_static;
use lwext4_rust::{Ext4File, InodeTypes as Ext4InodeTypes, bindings};
use spin::Mutex;

use crate::fs::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use crate::fs::{KStat, PageCache};
use crate::syscall::{Errno, SysResult};
use crate::timer::{TimeSpec, get_time_ms};

lazy_static! {
    static ref EXT4_INODE_CACHE: Mutex<HashMap<(usize, u64), Weak<dyn InodeOp>>> =
        Mutex::new(HashMap::new());
    static ref DEFERRED_INODE_DISCARDS: Mutex<Vec<(usize, &'static [u8], u32)>> =
        Mutex::new(Vec::new());
    /// lwext4 keeps mount, block-cache, and directory traversal state in
    /// shared C objects.  Every entry into lwext4, including superblock
    /// operations, must be serialized by this one lock on SMP.
    pub(super) static ref EXT4_OP_LOCK: ProfiledExt4Lock = ProfiledExt4Lock::new();
}

static DEFERRED_INODE_DISCARD_PENDING: AtomicBool = AtomicBool::new(false);

pub(super) struct ProfiledExt4Lock {
    inner: Mutex<()>,
}

#[derive(Clone, Copy)]
pub(super) enum Ext4LockClass {
    Stat,
    Lookup,
    Read,
    Write,
    Readdir,
    Namespace,
    Attributes,
    Superblock,
}

impl ProfiledExt4Lock {
    const fn new() -> Self {
        Self {
            inner: Mutex::new(()),
        }
    }

    pub(super) fn lock_class(&self, class: Ext4LockClass) -> ProfiledExt4Guard<'_> {
        let started = crate::perf::now_ticks();
        let guard = self.inner.lock();
        let waited = crate::perf::elapsed_since(started);
        crate::perf::observe_ext4_lock_wait(waited);
        match class {
            Ext4LockClass::Stat => {
                crate::perf::ext4_lock_stat_acquisition(1);
                crate::perf::ext4_lock_stat_wait_ticks(waited);
            }
            Ext4LockClass::Lookup => {
                crate::perf::ext4_lock_lookup_acquisition(1);
                crate::perf::ext4_lock_lookup_wait_ticks(waited);
            }
            Ext4LockClass::Read => {
                crate::perf::ext4_lock_read_acquisition(1);
                crate::perf::ext4_lock_read_wait_ticks(waited);
            }
            Ext4LockClass::Write => {
                crate::perf::ext4_lock_write_acquisition(1);
                crate::perf::ext4_lock_write_wait_ticks(waited);
            }
            Ext4LockClass::Readdir => {
                crate::perf::ext4_lock_readdir_acquisition(1);
                crate::perf::ext4_lock_readdir_wait_ticks(waited);
            }
            Ext4LockClass::Namespace => {
                crate::perf::ext4_lock_namespace_acquisition(1);
                crate::perf::ext4_lock_namespace_wait_ticks(waited);
            }
            Ext4LockClass::Attributes => {
                crate::perf::ext4_lock_attributes_acquisition(1);
                crate::perf::ext4_lock_attributes_wait_ticks(waited);
            }
            Ext4LockClass::Superblock => {
                crate::perf::ext4_lock_superblock_acquisition(1);
                crate::perf::ext4_lock_superblock_wait_ticks(waited);
            }
        }
        ProfiledExt4Guard {
            _guard: guard,
            acquired: crate::perf::now_ticks(),
            class,
        }
    }
}

impl Ext4LockClass {
    fn observe_lower(self, ticks: usize) {
        crate::perf::ext4_lower_call(1);
        crate::perf::ext4_lower_ticks(ticks);
        match self {
            Ext4LockClass::Stat => {
                crate::perf::ext4_lower_stat_call(1);
                crate::perf::ext4_lower_stat_ticks(ticks);
            }
            Ext4LockClass::Lookup => {
                crate::perf::ext4_lower_lookup_call(1);
                crate::perf::ext4_lower_lookup_ticks(ticks);
            }
            Ext4LockClass::Read => {
                crate::perf::ext4_lower_read_call(1);
                crate::perf::ext4_lower_read_ticks(ticks);
            }
            Ext4LockClass::Write => {
                crate::perf::ext4_lower_write_call(1);
                crate::perf::ext4_lower_write_ticks(ticks);
            }
            Ext4LockClass::Readdir => {
                crate::perf::ext4_lower_readdir_call(1);
                crate::perf::ext4_lower_readdir_ticks(ticks);
            }
            Ext4LockClass::Namespace => {
                crate::perf::ext4_lower_namespace_call(1);
                crate::perf::ext4_lower_namespace_ticks(ticks);
            }
            Ext4LockClass::Attributes => {
                crate::perf::ext4_lower_attributes_call(1);
                crate::perf::ext4_lower_attributes_ticks(ticks);
            }
            Ext4LockClass::Superblock => {
                crate::perf::ext4_lower_superblock_call(1);
                crate::perf::ext4_lower_superblock_ticks(ticks);
            }
        }
    }
}

pub(super) struct ProfiledExt4Guard<'a> {
    _guard: spin::MutexGuard<'a, ()>,
    acquired: usize,
    class: Ext4LockClass,
}

impl ProfiledExt4Guard<'_> {
    /// Profile one bounded lwext4 call or call sequence inside this lock.
    /// Keeping this explicit makes unprofiled Rust preparation/publication
    /// visible as the remainder of the existing lock-hold measurement.
    pub(super) fn profile_lower(&self) -> ProfiledExt4LowerGuard<'_> {
        ProfiledExt4LowerGuard {
            started: crate::perf::now_ticks(),
            class: self.class,
            _lock_guard: PhantomData,
        }
    }
}

pub(super) struct ProfiledExt4LowerGuard<'a> {
    started: usize,
    class: Ext4LockClass,
    _lock_guard: PhantomData<&'a ()>,
}

impl Drop for ProfiledExt4LowerGuard<'_> {
    fn drop(&mut self) {
        self.class
            .observe_lower(crate::perf::elapsed_since(self.started));
    }
}

impl Drop for ProfiledExt4Guard<'_> {
    fn drop(&mut self) {
        let held = crate::perf::elapsed_since(self.acquired);
        crate::perf::observe_ext4_lock_hold(held);
        match self.class {
            Ext4LockClass::Stat => crate::perf::ext4_lock_stat_hold_ticks(held),
            Ext4LockClass::Lookup => crate::perf::ext4_lock_lookup_hold_ticks(held),
            Ext4LockClass::Read => crate::perf::ext4_lock_read_hold_ticks(held),
            Ext4LockClass::Write => crate::perf::ext4_lock_write_hold_ticks(held),
            Ext4LockClass::Readdir => crate::perf::ext4_lock_readdir_hold_ticks(held),
            Ext4LockClass::Namespace => crate::perf::ext4_lock_namespace_hold_ticks(held),
            Ext4LockClass::Attributes => crate::perf::ext4_lock_attributes_hold_ticks(held),
            Ext4LockClass::Superblock => crate::perf::ext4_lock_superblock_hold_ticks(held),
        }
    }
}

pub struct Ext4Inode {
    fs_id: usize,
    mount_point: &'static str,
    mount_point_c: &'static [u8],
    pub ino: u64,
    ty: Ext4InodeTypes,
    metadata: Mutex<MetadataState>,
    unlinked: AtomicBool,
    /// 共享页缓存，挂载在 inode 上，同一 inode 的所有 File 共享
    page_cache: Arc<PageCache>,
}

struct MetadataState {
    raw: Option<RawInodeMetadata>,
    times: Option<InodeTimes>,
    generation: usize,
    pending_data_times: Option<PendingDataTimes>,
}

#[derive(Clone, Copy)]
struct PendingDataTimes {
    generation: usize,
    mtime: TimeSpec,
    ctime: TimeSpec,
}

#[derive(Clone, Copy)]
struct InodeTimes {
    atime: TimeSpec,
    mtime: TimeSpec,
    ctime: TimeSpec,
}

#[derive(Clone, Copy)]
struct RawInodeMetadata {
    mode: u32,
    uid: u32,
    gid: u32,
    atime: u32,
    mtime: u32,
    ctime: u32,
    size: u64,
    nlink: u32,
    namespace_generation: usize,
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        if self.unlinked.load(Ordering::Acquire) {
            DEFERRED_INODE_DISCARDS
                .lock()
                .push((self.fs_id, self.mount_point_c, self.ino as u32));
            DEFERRED_INODE_DISCARD_PENDING.store(true, Ordering::Release);
        }
    }
}

/// Reclaim an unlinked lower inode only after its final VFS `Arc` disappears.
/// Drop merely queues work so it never enters lwext4 under an unrelated lock.
pub fn reap_deferred_inodes() {
    if !DEFERRED_INODE_DISCARD_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }

    let pending = core::mem::take(&mut *DEFERRED_INODE_DISCARDS.lock());
    if pending.is_empty() {
        return;
    }

    {
        let mut cache = EXT4_INODE_CACHE.lock();
        for (fs_id, _, ino) in &pending {
            cache.remove(&(*fs_id, *ino as u64));
        }
    }

    let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Namespace);
    let mut retry = Vec::new();
    for (fs_id, mount, ino) in pending {
        let ret = {
            let _lower = guard.profile_lower();
            unsafe { bindings::ext4_inode_discard(mount.as_ptr().cast(), ino) }
        };
        if ret != 0 && ret != 22 {
            retry.push((fs_id, mount, ino));
        }
    }
    drop(guard);

    if !retry.is_empty() {
        DEFERRED_INODE_DISCARDS.lock().extend(retry);
        DEFERRED_INODE_DISCARD_PENDING.store(true, Ordering::Release);
    }
}

impl Ext4Inode {
    pub fn new(
        fs_id: usize,
        mount_point: &'static str,
        mount_point_c: &'static [u8],
        ino: u64,
        ty: Ext4InodeTypes,
    ) -> Self {
        Self {
            fs_id,
            mount_point,
            mount_point_c,
            ino,
            ty,
            metadata: Mutex::new(MetadataState {
                raw: None,
                times: None,
                generation: 1,
                pending_data_times: None,
            }),
            unlinked: AtomicBool::new(false),
            page_cache: PageCache::new(0),
        }
    }

    pub fn get_or_create(
        fs_id: usize,
        mount_point: &'static str,
        mount_point_c: &'static [u8],
        ino: u64,
        ty: Ext4InodeTypes,
    ) -> Arc<dyn InodeOp> {
        let mut cache = EXT4_INODE_CACHE.lock();
        let key = (fs_id, ino);
        if let Some(inode) = cache.get(&key).and_then(Weak::upgrade) {
            return inode;
        }

        // 缓存满则先清理已死亡的 Weak 条目
        if cache.len() >= INODE_CACHE_CAPACITY {
            evict_dead_inodes(&mut cache);
        }

        let inode: Arc<dyn InodeOp> =
            Arc::new(Self::new(fs_id, mount_point, mount_point_c, ino, ty));
        cache.insert(key, Arc::downgrade(&inode));
        inode
    }

    fn child_path(parent_path: &str, name: &str) -> alloc::string::String {
        if parent_path == "/" {
            alloc::format!("/{}", name)
        } else {
            alloc::format!("{}/{}", parent_path, name)
        }
    }

    fn check_type(&self, expected: InodeType) -> SysResult<()> {
        let actual = self.node_type();
        if actual == expected {
            Ok(())
        } else if expected == InodeType::Directory {
            Err(Errno::ENOTDIR)
        } else if actual == InodeType::Directory {
            Err(Errno::EISDIR)
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn map_lwext4_err(errno: i32) -> Errno {
        match errno {
            2 => Errno::ENOENT,
            5 => Errno::EIO,
            17 => Errno::EEXIST,
            20 => Errno::ENOTDIR,
            21 => Errno::EISDIR,
            22 => Errno::EINVAL,
            28 => Errno::ENOSPC,
            30 => Errno::EROFS,
            39 => Errno::ENOTEMPTY,
            16 => Errno::EBUSY,
            34 => Errno::ERANGE,
            61 => Errno::ENODATA,
            95 => Errno::EOPNOTSUPP,
            _ => Errno::EIO,
        }
    }

    fn file_link(old_path: &str, hardlink_path: &str) -> SysResult {
        let old_path = CString::new(old_path).map_err(|_| Errno::EINVAL)?;
        let new_path = CString::new(hardlink_path).map_err(|_| Errno::EINVAL)?;
        {
            let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Namespace);
            let ret = {
                let _lower = guard.profile_lower();
                unsafe { bindings::ext4_flink(old_path.as_ptr(), new_path.as_ptr()) }
            };
            if ret != 0 {
                return Err(Self::map_lwext4_err(ret));
            }
        }
        Ok(())
    }

    fn file_symlink(target: &str, path: &str) -> SysResult {
        // Validate and allocate both immutable arguments before entering the
        // global lwext4 critical section. Failure still precedes mutation.
        let target = CString::new(target).map_err(|_| Errno::EINVAL)?;
        let path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        {
            let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Namespace);
            // lwext4 负责选择 fast symlink 或普通数据块存储；VFS 层只传入目标字符串和新路径。
            let ret = {
                let _lower = guard.profile_lower();
                unsafe { bindings::ext4_fsymlink(target.as_ptr(), path.as_ptr()) }
            };
            if ret != 0 {
                return Err(Self::map_lwext4_err(ret));
            }
        }
        Ok(())
    }

    pub(crate) fn file_rename(old_path: &str, new_path: &str) -> SysResult {
        let c_old = CString::new(old_path).map_err(|_| Errno::EINVAL)?;
        let c_new = CString::new(new_path).map_err(|_| Errno::EINVAL)?;
        {
            let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Namespace);
            let ret = {
                let _lower = guard.profile_lower();
                unsafe { bindings::ext4_frename(c_old.as_ptr(), c_new.as_ptr()) }
            };
            if ret != 0 {
                return Err(Self::map_lwext4_err(ret));
            }
        }
        Ok(())
    }

    pub(crate) fn remove_path(path: &str, ty: InodeType, deferred: bool) -> SysResult {
        let path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Namespace);
        let ret = {
            let _lower = guard.profile_lower();
            unsafe {
                match (ty, deferred) {
                    (InodeType::Directory, true) => bindings::ext4_dir_rm_deferred(path.as_ptr()),
                    (InodeType::Directory, false) => bindings::ext4_dir_rm(path.as_ptr()),
                    (_, true) => bindings::ext4_fremove_deferred(path.as_ptr()),
                    (_, false) => bindings::ext4_fremove(path.as_ptr()),
                }
            }
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(Self::map_lwext4_err(ret))
        }
    }

    pub(crate) fn namespace_changed(&self) {
        let mut metadata = self.metadata.lock();
        metadata.generation = metadata.generation.wrapping_add(1);
        metadata.raw = None;
    }

    pub(crate) fn mark_unlinked(&self) {
        self.unlinked.store(true, Ordering::Release);
        self.invalidate_raw_metadata();
    }

    pub(crate) fn is_unlinked(&self) -> bool {
        self.unlinked.load(Ordering::Acquire)
    }

    fn dirent64_reclen(name_len: usize) -> usize {
        // 目录项固定字段大小
        const DIRENT64_HEADER_SIZE: usize = 8 + 8 + 2 + 1;
        // 变长文件名字段大小
        ((DIRENT64_HEADER_SIZE + name_len + 1) + 7) & !7 // 对齐 8 字节
    }

    fn lookup_dirent(parent_path: &str, name: &str) -> SysResult<(u64, Ext4InodeTypes)> {
        let child_path = Self::child_path(parent_path, name);
        let c_path = CString::new(child_path).map_err(|_| Errno::EINVAL)?;
        let mut ino = 0u32;
        let mut raw_inode: bindings::ext4_inode = unsafe { core::mem::zeroed() };
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Lookup);
        let ret = {
            let _lower = guard.profile_lower();
            unsafe { bindings::ext4_raw_inode_fill(c_path.as_ptr(), &mut ino, &mut raw_inode) }
        };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        let mode = u16::from_le(raw_inode.mode) as usize & 0o170000;
        Ok((ino as u64, Ext4InodeTypes::from(mode)))
    }

    fn inode_mode_type(path: &str) -> Option<Ext4InodeTypes> {
        let c_path = CString::new(path).ok()?;
        let c_path = c_path.into_raw();
        let mut mode = 0;
        let started = crate::perf::now_ticks();
        let ret = unsafe { bindings::ext4_mode_get(c_path, &mut mode) };
        Ext4LockClass::Readdir.observe_lower(crate::perf::elapsed_since(started));
        unsafe {
            drop(CString::from_raw(c_path));
        }
        if ret != 0 {
            return None;
        }
        Some(Ext4InodeTypes::from((mode & 0o170000) as usize))
    }

    fn now_timespec() -> TimeSpec {
        let ms = get_time_ms();
        TimeSpec {
            sec: (ms / 1000) as isize,
            nsec: ((ms % 1000) * 1_000_000) as isize,
        }
    }

    fn normalize_lower_time(sec: u32) -> TimeSpec {
        let now = Self::now_timespec();
        let ts = TimeSpec {
            sec: sec as isize,
            nsec: 0,
        };
        // TODO[ABI-COMPAT]: 当前 CLOCK_REALTIME 仍是开机时间，镜像里的 ext4
        // 时间戳却是构建机 Unix 时间。没有内核侧覆盖记录时，先把“未来”
        // 时间归零，避免 libc 用 time() 与 stat() 比较时失真。
        if ts.sec > now.sec {
            TimeSpec::default()
        } else {
            ts
        }
    }

    fn cached_times(&self, atime: u32, mtime: u32, ctime: u32) -> InodeTimes {
        if let Some(times) = self.metadata.lock().times {
            return times;
        }
        InodeTimes {
            atime: Self::normalize_lower_time(atime),
            mtime: Self::normalize_lower_time(mtime),
            ctime: Self::normalize_lower_time(ctime),
        }
    }

    fn current_times(&self, path: &str) -> SysResult<InodeTimes> {
        if let Some(times) = self.metadata.lock().times {
            return Ok(times);
        }
        self.stat(path).map(|stat| InodeTimes {
            atime: stat.atime,
            mtime: stat.mtime,
            ctime: stat.ctime,
        })
    }

    fn commit_lower_setattr(
        &self,
        _path: &str,
        mode: Option<u32>,
        owner: Option<(u32, u32)>,
        atime: Option<TimeSpec>,
        mtime: Option<TimeSpec>,
        ctime: Option<TimeSpec>,
    ) -> SysResult {
        let lower_time = |time: Option<TimeSpec>| {
            time.filter(|time| time.sec >= 0 && time.sec <= u32::MAX as isize)
                .map(|time| time.sec as u32)
        };
        let lower_atime = lower_time(atime);
        let lower_mtime = lower_time(mtime);
        let lower_ctime = lower_time(ctime);
        let mask = u32::from(mode.is_some())
            | (u32::from(owner.is_some()) << 1)
            | (u32::from(lower_atime.is_some()) << 2)
            | (u32::from(lower_mtime.is_some()) << 3)
            | (u32::from(lower_ctime.is_some()) << 4);
        let (uid, gid) = owner.unwrap_or_default();
        let mount = self.mount_point_c;
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Attributes);
        let ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_setattr_ino(
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    mask,
                    mode.unwrap_or(0) & 0o7777,
                    uid,
                    gid,
                    lower_atime.unwrap_or(0),
                    lower_mtime.unwrap_or(0),
                    lower_ctime.unwrap_or(0),
                )
            }
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(Self::map_lwext4_err(ret))
        }
    }

    fn set_times_impl(
        &self,
        path: &str,
        atime: Option<TimeSpec>,
        mtime: Option<TimeSpec>,
        update_ctime: bool,
    ) -> SysResult {
        // Preserve an earlier delayed write's mtime before a later explicit
        // setattr supersedes some or all timestamp fields.
        self.flush_pending_data_times(path)?;
        let mut times = self.current_times(path)?;

        if let Some(atime) = atime {
            times.atime = atime;
        }
        if let Some(mtime) = mtime {
            times.mtime = mtime;
        }
        if update_ctime {
            times.ctime = Self::now_timespec();
        }

        crate::perf::ext4_set_times_call(1);
        if atime.is_some() {
            crate::perf::ext4_set_times_atime_update(1);
        }
        if mtime.is_some() {
            crate::perf::ext4_set_times_mtime_update(1);
        }
        self.commit_lower_setattr(
            path,
            None,
            None,
            atime,
            mtime,
            update_ctime.then_some(times.ctime),
        )?;
        self.set_cached_times(times);
        Ok(())
    }

    /// Automatic read/readdir atime updates do not change ctime. Explicit
    /// timestamp changes continue through `InodeOp::set_times` and do.
    pub(crate) fn touch_atime(&self, path: &str, atime: TimeSpec) -> SysResult {
        self.set_times_impl(path, Some(atime), None, false)
    }

    fn init_inode_times(&self) {
        let now = Self::now_timespec();
        self.metadata.lock().times = Some(InodeTimes {
            atime: now,
            mtime: now,
            ctime: now,
        });
    }

    fn set_cached_times(&self, times: InodeTimes) {
        self.metadata.lock().times = Some(times);
    }

    fn flush_pending_data_times(&self, path: &str) -> SysResult {
        let pending = self.metadata.lock().pending_data_times;
        let Some(pending) = pending else {
            return Ok(());
        };
        crate::perf::ext4_set_times_call(1);
        crate::perf::ext4_set_times_mtime_update(1);
        self.commit_lower_setattr(
            path,
            None,
            None,
            None,
            Some(pending.mtime),
            Some(pending.ctime),
        )?;
        let mut metadata = self.metadata.lock();
        if metadata
            .pending_data_times
            .is_some_and(|current| current.generation == pending.generation)
        {
            metadata.pending_data_times = None;
        }
        Ok(())
    }

    pub(crate) fn invalidate_raw_metadata(&self) {
        if self.metadata.lock().raw.take().is_some() {
            crate::perf::ext4_stat_cache_invalidation(1);
        }
    }

    fn raw_metadata(&self, _path: &str) -> SysResult<RawInodeMetadata> {
        let ty = self.node_type();
        let cacheable = matches!(
            ty,
            InodeType::Regular | InodeType::SymLink | InodeType::Directory
        );
        let mut state = self.metadata.lock();
        let namespace_generation = state.generation;
        if cacheable {
            if let Some(metadata) = state.raw {
                if ty != InodeType::Directory
                    || metadata.namespace_generation == namespace_generation
                {
                    crate::perf::ext4_stat_cache_hit(1);
                    return Ok(metadata);
                }
                state.raw = None;
                crate::perf::ext4_stat_cache_invalidation(1);
            }
        }
        crate::perf::ext4_stat_cache_miss(1);
        if cacheable {
            crate::perf::ext4_stat_cache_refill(1);
        } else {
            crate::perf::ext4_stat_cache_uncacheable(1);
        }

        drop(state);
        let raw_inode = {
            let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Stat);
            let mut raw_inode: bindings::ext4_inode = unsafe { core::mem::zeroed() };
            let mount = self.mount_point_c;
            let ret = {
                let _lower = guard.profile_lower();
                unsafe {
                    bindings::ext4_raw_inode_fill_ino(
                        mount.as_ptr().cast(),
                        self.ino as u32,
                        &mut raw_inode,
                    )
                }
            };
            if ret != 0 {
                return Err(Self::map_lwext4_err(ret));
            }
            raw_inode
        };
        // Reload after lower I/O so a mutation of this directory cannot make
        // a fresh snapshot carry the generation observed before the I/O.
        let mut state = self.metadata.lock();
        let namespace_generation = state.generation;

        let osd2 = unsafe { core::ptr::addr_of!(raw_inode.osd2).read_unaligned() };
        let linux2 = unsafe { osd2.linux2 };
        let metadata = RawInodeMetadata {
            mode: u16::from_le(raw_inode.mode) as u32,
            uid: u16::from_le(raw_inode.uid) as u32
                | ((u16::from_le(linux2.uid_high) as u32) << 16),
            gid: u16::from_le(raw_inode.gid) as u32
                | ((u16::from_le(linux2.gid_high) as u32) << 16),
            atime: u32::from_le(raw_inode.access_time),
            mtime: u32::from_le(raw_inode.modification_time),
            ctime: u32::from_le(raw_inode.change_inode_time),
            size: (u32::from_le(raw_inode.size_lo) as u64)
                | ((u32::from_le(raw_inode.size_hi) as u64) << 32),
            nlink: u16::from_le(raw_inode.links_count) as u32,
            namespace_generation,
        };
        if cacheable {
            state.raw = Some(metadata);
        }
        Ok(metadata)
    }
}

impl InodeOp for Ext4Inode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::from(self.ty.clone())
    }

    fn get_page_cache(&self) -> Option<Arc<PageCache>> {
        if self.node_type() == InodeType::Regular {
            Some(self.page_cache.clone())
        } else {
            None
        }
    }

    fn stat(&self, path: &str) -> SysResult<KStat> {
        let started = crate::perf::now_ticks();
        let ty = self.node_type();
        let ino = self.ino;

        let raw = self.raw_metadata(path)?;
        let times = self.cached_times(raw.atime, raw.mtime, raw.ctime);
        let size = match ty {
            InodeType::Regular => (raw.size as usize).max(self.page_cache.len()),
            InodeType::SymLink => raw.size as usize,
            _ => 0,
        };

        let stat = KStat {
            dev: 0,
            size,
            ty,
            ino,
            nlink: raw.nlink,
            uid: raw.uid,
            gid: raw.gid,
            rdev: 0,
            mode: raw.mode,
            mode_valid: true,
            blksize: crate::config::BLOCK_SIZE as u32,
            blocks: KStat::blocks_for_size(size as u64),
            atime: times.atime,
            mtime: times.mtime,
            ctime: times.ctime,
        };
        crate::perf::ext4_stat_call(1);
        crate::perf::ext4_stat_ticks(crate::perf::elapsed_since(started));
        Ok(stat)
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;
        crate::perf::inode_read_call(1);
        crate::perf::inode_read_requested_bytes(buf.len());

        let started = crate::perf::now_ticks();
        let read_size = {
            let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Read);
            let file = &mut Ext4File::new(self.mount_point, self.ty.clone());
            {
                let _lower = guard.profile_lower();
                file.inode_open(self.ino as u32, bindings::O_RDONLY)
                    .map_err(Self::map_lwext4_err)?;
                file.file_seek(off as i64, bindings::SEEK_SET)
                    .map_err(Self::map_lwext4_err)?;
            }
            // lwext4 advances across sparse extents but does not reliably
            // write zeroes into every byte of the caller's buffer.  POSIX hole
            // reads must return zero, and file-backed mmap depends on that.
            buf.fill(0);
            let read_size = {
                let _lower = guard.profile_lower();
                let read_size = file.file_read(buf).map_err(Self::map_lwext4_err)?;
                file.file_close().map_err(Self::map_lwext4_err)?;
                read_size
            };
            read_size
        };
        // lwext4 may update atime while reading.  Invalidate only after the
        // global ext4 lock is released to preserve the cache/EXT4 lock order.
        self.invalidate_raw_metadata();
        crate::perf::inode_read_completed_bytes(read_size);
        crate::perf::inode_read_ticks(crate::perf::elapsed_since(started));

        Ok(read_size)
    }

    fn write_at(&self, _path: &str, off: usize, buf: &[u8]) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;
        let started = crate::perf::now_ticks();

        let write_size = {
            let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Write);
            let file = &mut Ext4File::new(self.mount_point, self.ty.clone());
            let _lower = guard.profile_lower();
            file.inode_open(self.ino as u32, bindings::O_RDWR)
                .map_err(Self::map_lwext4_err)?;
            // lwext4's fseek rejects offsets beyond EOF and the Rust wrapper
            // historically clamped them to the current size.  Page-cache
            // writeback may legitimately start a dirty extent after a sparse
            // hole, so extend the lower file before positioning the write.
            if off as u64 > file.file_size() {
                file.file_truncate(off as u64)
                    .map_err(Self::map_lwext4_err)?;
            }
            file.file_seek(off as i64, bindings::SEEK_SET)
                .map_err(Self::map_lwext4_err)?;
            let write_size = file.file_write(buf).map_err(Self::map_lwext4_err)?;
            file.file_close().map_err(Self::map_lwext4_err)?;
            write_size
        };
        self.invalidate_raw_metadata();

        if write_size != 0 {
            let end = off.checked_add(write_size).ok_or(Errno::EINVAL)?;
            if end > self.page_cache.len() {
                self.page_cache.resize(end);
            }
        }

        crate::perf::ext4_write_call(1);
        crate::perf::ext4_write_ticks(crate::perf::elapsed_since(started));
        Ok(write_size)
    }

    fn truncate(&self, path: &str, size: usize) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;

        self.page_cache.with_writeback_exclusion(|| {
            {
                let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Write);
                let file = &mut Ext4File::new(self.mount_point, self.ty.clone());
                let _lower = guard.profile_lower();
                file.inode_open(self.ino as u32, bindings::O_RDWR)
                    .map_err(Self::map_lwext4_err)?;
                file.file_truncate(size as u64)
                    .map_err(Self::map_lwext4_err)?;
                file.file_close().map_err(Self::map_lwext4_err)?;
            }
            self.invalidate_raw_metadata();

            let now = Self::now_timespec();
            self.set_times(path, None, Some(now))?;
            self.page_cache.resize(size);
            Ok(())
        })?;
        crate::fs::release_clean_owner(&self.page_cache);

        Ok(0)
    }

    fn note_data_write(&self, path: &str, time: TimeSpec) -> SysResult {
        let mut times = self.current_times(path)?;
        times.mtime = time;
        times.ctime = time;
        let mut metadata = self.metadata.lock();
        let generation = metadata
            .pending_data_times
            .map_or(1, |pending| pending.generation.wrapping_add(1));
        metadata.times = Some(times);
        metadata.pending_data_times = Some(PendingDataTimes {
            generation,
            mtime: time,
            ctime: time,
        });
        Ok(())
    }

    fn flush_data_metadata(&self, path: &str) -> SysResult {
        self.flush_pending_data_times(path)
    }

    fn has_pending_data_metadata(&self) -> bool {
        self.metadata.lock().pending_data_times.is_some()
    }

    fn set_times(&self, path: &str, atime: Option<TimeSpec>, mtime: Option<TimeSpec>) -> SysResult {
        self.set_times_impl(path, atime, mtime, true)
    }

    fn set_mode(&self, path: &str, mode: u32) -> SysResult {
        self.flush_pending_data_times(path)?;
        crate::perf::ext4_set_mode_call(1);
        let mut times = self.current_times(path)?;
        times.ctime = Self::now_timespec();
        self.commit_lower_setattr(path, Some(mode), None, None, None, Some(times.ctime))?;
        self.invalidate_raw_metadata();
        self.set_cached_times(times);
        Ok(())
    }

    fn set_owner(&self, path: &str, uid: u32, gid: u32) -> SysResult {
        self.flush_pending_data_times(path)?;
        crate::perf::ext4_set_owner_call(1);
        let mut times = self.current_times(path)?;
        times.ctime = Self::now_timespec();
        self.commit_lower_setattr(path, None, Some((uid, gid)), None, None, Some(times.ctime))?;
        self.invalidate_raw_metadata();
        self.set_cached_times(times);
        Ok(())
    }

    fn set_owner_and_mode(&self, path: &str, uid: u32, gid: u32, mode: Option<u32>) -> SysResult {
        self.flush_pending_data_times(path)?;
        crate::perf::ext4_set_owner_call(1);
        if mode.is_some() {
            crate::perf::ext4_set_mode_call(1);
        }
        let mut times = self.current_times(path)?;
        times.ctime = Self::now_timespec();
        self.commit_lower_setattr(path, mode, Some((uid, gid)), None, None, Some(times.ctime))?;
        self.invalidate_raw_metadata();
        self.set_cached_times(times);
        Ok(())
    }

    fn set_xattr(&self, name: String, value: Vec<u8>, flags: usize) -> SysResult {
        const XATTR_CREATE: usize = 0x1;
        const XATTR_REPLACE: usize = 0x2;

        let exists = match self.get_xattr(&name) {
            Ok(_) => true,
            Err(Errno::ENODATA) => false,
            Err(error) => return Err(error),
        };
        if flags & XATTR_CREATE != 0 && exists {
            return Err(Errno::EEXIST);
        }
        if flags & XATTR_REPLACE != 0 && !exists {
            return Err(Errno::ENODATA);
        }
        let name = CString::new(name).map_err(|_| Errno::EINVAL)?;
        let mount = self.mount_point_c;
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Attributes);
        let ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_setxattr_ino(
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    value.as_ptr().cast(),
                    value.len(),
                )
            }
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(Self::map_lwext4_err(ret))
        }
    }

    fn get_xattr(&self, name: &str) -> Result<Vec<u8>, Errno> {
        let name = CString::new(name).map_err(|_| Errno::EINVAL)?;
        let mount = self.mount_point_c;
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Attributes);
        let mut size = 0usize;
        let mut ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_getxattr_ino(
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    core::ptr::null_mut(),
                    0,
                    &mut size,
                )
            }
        };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        let mut value = vec![0u8; size];
        ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_getxattr_ino(
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    &mut size,
                )
            }
        };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        value.truncate(size);
        Ok(value)
    }

    fn list_xattr(&self) -> Result<Vec<String>, Errno> {
        let mount = self.mount_point_c;
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Attributes);
        let mut size = 0usize;
        let mut ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_listxattr_ino(
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    core::ptr::null_mut(),
                    0,
                    &mut size,
                )
            }
        };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        let mut list = vec![0u8; size];
        ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_listxattr_ino(
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    list.as_mut_ptr().cast(),
                    list.len(),
                    &mut size,
                )
            }
        };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        list.truncate(size);
        list.split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| {
                core::str::from_utf8(name)
                    .map(String::from)
                    .map_err(|_| Errno::EINVAL)
            })
            .collect()
    }

    fn remove_xattr(&self, name: &str) -> SysResult {
        let name = CString::new(name).map_err(|_| Errno::EINVAL)?;
        let mount = self.mount_point_c;
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Attributes);
        let ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_removexattr_ino(
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    name.as_ptr(),
                    name.as_bytes().len(),
                )
            }
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(Self::map_lwext4_err(ret))
        }
    }

    fn clear_xattrs(&self) {}

    /// 查找与 name 匹配的子索引节点，约定 name 为常规文件名
    fn lookup(&self, parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;
        let started = crate::perf::now_ticks();

        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(Errno::EINVAL);
        }

        let (child_ino, child_ty) = Self::lookup_dirent(parent_path, name)?;
        crate::perf::ext4_lookup_call(1);
        crate::perf::ext4_lookup_ticks(crate::perf::elapsed_since(started));
        Ok(Self::get_or_create(
            self.fs_id,
            self.mount_point,
            self.mount_point_c,
            child_ino,
            child_ty,
        ))
    }

    fn readdir(&self, path: &str) -> SysResult<Vec<LinuxDirent64>> {
        self.check_type(InodeType::Directory)?;
        let started = crate::perf::now_ticks();

        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Readdir);
        let mut dir: bindings::ext4_dir = unsafe { core::mem::zeroed() };
        let mount = self.mount_point_c;
        let ret = {
            let _lower = guard.profile_lower();
            unsafe {
                bindings::ext4_inode_open(
                    &mut dir.f,
                    mount.as_ptr().cast(),
                    self.ino as u32,
                    bindings::O_RDONLY,
                )
            }
        };
        dir.next_off = 0;
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }

        let mut entries = Vec::new();
        let mut next_off = 0usize;

        loop {
            let dirent = {
                let _lower = guard.profile_lower();
                unsafe { bindings::ext4_dir_entry_next(&mut dir) }
            };
            if dirent.is_null() {
                break;
            }

            let dirent = unsafe { &*dirent };
            if dirent.inode == 0 {
                continue;
            }
            let name_len = dirent.name_length as usize;
            let name = core::str::from_utf8(&dirent.name[..name_len]).map_err(|_| Errno::EINVAL)?;
            let dirent_ty = Ext4InodeTypes::from(dirent.inode_type as usize);
            let ty = if dirent_ty != Ext4InodeTypes::EXT4_DE_UNKNOWN {
                crate::perf::ext4_readdir_dirent_type_known(1);
                InodeType::from(dirent_ty)
            } else if name == "." || name == ".." {
                // DT_UNKNOWN is a valid result when the filesystem does not
                // store file types in directory entries.  Preserve it for the
                // synthetic self/parent entries rather than constructing an
                // alias-sensitive fallback path for `..`.
                crate::perf::ext4_readdir_dirent_type_unknown(1);
                InodeType::Unknown
            } else {
                crate::perf::ext4_readdir_dirent_type_unknown(1);
                let child_path = Self::child_path(path, name);
                let Some(ty) = Self::inode_mode_type(&child_path) else {
                    continue;
                };
                InodeType::from(ty)
            };
            let reclen = Self::dirent64_reclen(name_len);
            next_off += reclen;

            let mut d_name = dirent.name[..name_len].to_vec();
            d_name.push(0);
            entries.push(LinuxDirent64 {
                d_ino: dirent.inode as u64,
                d_off: next_off as i64,
                d_reclen: reclen as u16,
                d_type: ty as u8,
                d_name,
            });
        }

        let ret = {
            let _lower = guard.profile_lower();
            unsafe { bindings::ext4_dir_close(&mut dir) }
        };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        drop(guard);
        // Directory iteration may update atime in lwext4.  Do not retain a
        // raw timestamp snapshot across a successful readdir operation.
        self.invalidate_raw_metadata();

        crate::perf::ext4_readdir_call(1);
        crate::perf::ext4_readdir_ticks(crate::perf::elapsed_since(started));
        Ok(entries)
    }

    fn create(&self, parent_path: &str, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;
        let started = crate::perf::now_ticks();

        let path = Self::child_path(parent_path, name);
        let ext4_ty = Ext4InodeTypes::from(ty);
        let c_path = CString::new(path.as_str()).map_err(|_| Errno::EINVAL)?;
        {
            let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Namespace);
            let file = &mut Ext4File::new(parent_path, self.ty.clone());
            let _lower = guard.profile_lower();

            if file.check_inode_exist(&path, ext4_ty.clone()) {
                return Err(Errno::EEXIST);
            }

            let new_file = &mut Ext4File::new(&path, ext4_ty.clone());

            match ext4_ty {
                Ext4InodeTypes::EXT4_DE_DIR => {
                    new_file.dir_mk(&path).map_err(Self::map_lwext4_err)?;
                }
                Ext4InodeTypes::EXT4_DE_REG_FILE => {
                    new_file
                        .file_open(
                            &path,
                            bindings::O_RDWR | bindings::O_CREAT | bindings::O_TRUNC,
                        )
                        .map_err(Self::map_lwext4_err)?;
                    new_file.file_close().map_err(Self::map_lwext4_err)?;
                }
                Ext4InodeTypes::EXT4_DE_FIFO => {
                    let ret = unsafe {
                        bindings::ext4_mknod(c_path.as_ptr(), bindings::EXT4_DE_FIFO as i32, 0)
                    };
                    if ret != 0 {
                        return Err(Self::map_lwext4_err(ret));
                    }
                }
                Ext4InodeTypes::EXT4_DE_CHRDEV
                | Ext4InodeTypes::EXT4_DE_BLKDEV
                | Ext4InodeTypes::EXT4_DE_SOCK => {
                    new_file
                        .file_open(
                            &path,
                            bindings::O_RDWR | bindings::O_CREAT | bindings::O_TRUNC,
                        )
                        .map_err(Self::map_lwext4_err)?;
                    new_file.file_close().map_err(Self::map_lwext4_err)?;
                }
                _ => return Err(Errno::ENOSYS),
            }
        }
        self.namespace_changed();

        let (child_ino, child_ty) = Self::lookup_dirent(parent_path, name)?;
        let inode = Self::get_or_create(
            self.fs_id,
            self.mount_point,
            self.mount_point_c,
            child_ino,
            child_ty,
        );
        if let Some(inode) = inode.as_any().downcast_ref::<Ext4Inode>() {
            inode.init_inode_times();
        }
        crate::perf::ext4_create_call(1);
        crate::perf::ext4_create_ticks(crate::perf::elapsed_since(started));
        Ok(inode)
    }

    fn symlink(&self, target: &str, parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;

        let path = Self::child_path(parent_path, name);
        Self::file_symlink(target, &path)?;
        self.namespace_changed();
        // 创建后重新 lookup，复用现有 inode cache/type 修正逻辑。
        let inode = self.lookup(parent_path, name)?;
        inode.clear_xattrs();
        if let Some(inode) = inode.as_any().downcast_ref::<Ext4Inode>() {
            inode.init_inode_times();
        }
        Ok(inode)
    }

    fn link(&self, old_path: &str, bare_dentry: Arc<Dentry>) -> SysResult {
        // 调用者保证参数合法
        // if self.node_type() == InodeType::Directory {
        //     return Err(Errno::EPERM);
        // }
        // if bare_dentry.try_get_inode().is_some() {
        //     return Err(Errno::EEXIST);
        // }

        Self::file_link(old_path, &bare_dentry.abs_path)?;
        if let Some(parent) = bare_dentry.get_parent() {
            if let Some(parent) = parent.get_inode().as_any().downcast_ref::<Ext4Inode>() {
                parent.namespace_changed();
            }
        }
        self.invalidate_raw_metadata();
        Ok(())
    }

    fn unlink(&self, valid_dentry: &Arc<Dentry>) -> SysResult {
        // 调用者保证参数合法
        // self.check_type(InodeType::Directory)?;

        let current_path = valid_dentry.current_abs_path();
        info!("[kernel] unlink: {}", current_path);

        let child_abs_path = &current_path;
        let child_inode = valid_dentry.try_get_inode().ok_or(Errno::ENOENT)?;
        let child_ext4 = child_inode
            .as_any()
            .downcast_ref::<Ext4Inode>()
            .ok_or(Errno::EXDEV)?;
        let child_stat = child_inode.stat(child_abs_path)?;
        // `i_nlink == 0` detaches the inode from the namespace. Lower storage
        // is reclaimed only after every VFS Arc (File, cwd, Dentry and
        // transient Path) has disappeared.
        let deferred = child_inode.node_type() == InodeType::Directory || child_stat.nlink <= 1;
        if child_inode.node_type() == InodeType::Directory {
            let entries = child_inode.readdir(child_abs_path)?;
            let has_content = entries
                .iter()
                .any(|e| e.d_name != b".\0" && e.d_name != b"..\0");
            if has_content {
                return Err(Errno::ENOTEMPTY);
            }
            Self::remove_path(child_abs_path, InodeType::Directory, deferred)?;
        } else {
            Self::remove_path(child_abs_path, child_inode.node_type(), deferred)?;
        };
        child_ext4.invalidate_raw_metadata();
        if deferred {
            child_ext4.mark_unlinked();
        }
        self.namespace_changed();
        Ok(())
    }

    fn read_link(&self, _path: &str) -> SysResult<String> {
        // readlinkat 必须作用在 symlink inode 自身，传到这里的 path 不应已经被 namei 跟随。
        if self.node_type() != InodeType::SymLink {
            return Err(Errno::EINVAL);
        }
        const MAX_LINK_TARGET: usize = 4096;
        let guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Read);
        let file = &mut Ext4File::new(self.mount_point, self.ty.clone());
        let _lower = guard.profile_lower();
        file.inode_open(self.ino as u32, bindings::O_RDONLY)
            .map_err(Self::map_lwext4_err)?;
        let mut buf = vec![0u8; MAX_LINK_TARGET];
        let size = file.file_read(&mut buf).map_err(Self::map_lwext4_err)?;
        file.file_close().map_err(Self::map_lwext4_err)?;
        buf.truncate(size);
        String::from_utf8(buf).map_err(|_| Errno::EINVAL)
    }
}

/// 清除缓存中已死亡的 Weak 条目
fn evict_dead_inodes(cache: &mut HashMap<(usize, u64), Weak<dyn InodeOp>>) {
    let dead: Vec<(usize, u64)> = cache
        .iter()
        .filter(|(_, w)| w.upgrade().is_none())
        .map(|(k, _)| *k)
        .collect();
    for key in dead {
        cache.remove(&key);
    }
}

/// 手动清理 inode 缓存中已死亡的条目
pub fn clean_inode_cache() {
    evict_dead_inodes(&mut EXT4_INODE_CACHE.lock());
}

impl From<InodeType> for Ext4InodeTypes {
    fn from(ty: InodeType) -> Self {
        match ty {
            InodeType::Unknown => Ext4InodeTypes::EXT4_DE_UNKNOWN,
            InodeType::Fifo => Ext4InodeTypes::EXT4_DE_FIFO,
            InodeType::CharDevice => Ext4InodeTypes::EXT4_DE_CHRDEV,
            InodeType::Directory => Ext4InodeTypes::EXT4_DE_DIR,
            InodeType::BlockDevice => Ext4InodeTypes::EXT4_DE_BLKDEV,
            InodeType::Regular => Ext4InodeTypes::EXT4_DE_REG_FILE,
            InodeType::SymLink => Ext4InodeTypes::EXT4_DE_SYMLINK,
            InodeType::Socket => Ext4InodeTypes::EXT4_DE_SOCK,
        }
    }
}

impl From<Ext4InodeTypes> for InodeType {
    fn from(ty: Ext4InodeTypes) -> Self {
        match ty {
            Ext4InodeTypes::EXT4_DE_UNKNOWN => InodeType::Unknown,
            Ext4InodeTypes::EXT4_DE_FIFO | Ext4InodeTypes::EXT4_INODE_MODE_FIFO => InodeType::Fifo,
            Ext4InodeTypes::EXT4_DE_CHRDEV | Ext4InodeTypes::EXT4_INODE_MODE_CHARDEV => {
                InodeType::CharDevice
            }
            Ext4InodeTypes::EXT4_DE_DIR | Ext4InodeTypes::EXT4_INODE_MODE_DIRECTORY => {
                InodeType::Directory
            }
            Ext4InodeTypes::EXT4_DE_BLKDEV | Ext4InodeTypes::EXT4_INODE_MODE_BLOCKDEV => {
                InodeType::BlockDevice
            }
            Ext4InodeTypes::EXT4_DE_REG_FILE | Ext4InodeTypes::EXT4_INODE_MODE_FILE => {
                InodeType::Regular
            }
            Ext4InodeTypes::EXT4_DE_SYMLINK | Ext4InodeTypes::EXT4_INODE_MODE_SOFTLINK => {
                InodeType::SymLink
            }
            Ext4InodeTypes::EXT4_DE_SOCK | Ext4InodeTypes::EXT4_INODE_MODE_SOCKET => {
                InodeType::Socket
            }
            _ => InodeType::Unknown,
        }
    }
}
