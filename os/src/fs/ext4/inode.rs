// os/src/ext4/inode.rs

use crate::config::INODE_CACHE_CAPACITY;
use alloc::ffi::CString;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use hashbrown::HashMap;
use lazy_static::lazy_static;
use lwext4_rust::{Ext4File, InodeTypes as Ext4InodeTypes, bindings};
use spin::Mutex;

use crate::fs::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use crate::fs::{KStat, PageCache};
use crate::syscall::{Errno, SysResult};
use crate::timer::{TimeSpec, get_time_ms};

lazy_static! {
    static ref EXT4_INODE_CACHE: Mutex<HashMap<u64, Weak<dyn InodeOp>>> =
        Mutex::new(HashMap::new());
    static ref EXT4_OP_LOCK: Mutex<()> = Mutex::new(());
}

pub struct Ext4Inode {
    pub ino: u64,
    ty: Ext4InodeTypes,
    times: Mutex<Option<InodeTimes>>,
    /// 共享页缓存，挂载在 inode 上，同一 inode 的所有 File 共享
    page_cache: Arc<PageCache>,
}

#[derive(Clone, Copy)]
struct InodeTimes {
    atime: TimeSpec,
    mtime: TimeSpec,
    ctime: TimeSpec,
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode {
    pub fn new(ino: u64, ty: Ext4InodeTypes) -> Self {
        Self {
            ino,
            ty,
            times: Mutex::new(None),
            page_cache: PageCache::new(0),
        }
    }

    pub fn get_or_create(ino: u64, ty: Ext4InodeTypes) -> Arc<dyn InodeOp> {
        let mut cache = EXT4_INODE_CACHE.lock();
        if let Some(inode) = cache.get(&ino).and_then(Weak::upgrade) {
            return inode;
        }

        // 缓存满则先清理已死亡的 Weak 条目
        if cache.len() >= INODE_CACHE_CAPACITY {
            evict_dead_inodes(&mut cache);
        }

        let inode: Arc<dyn InodeOp> = Arc::new(Self::new(ino, ty));
        cache.insert(ino, Arc::downgrade(&inode));
        inode
    }

    fn child_path(parent_path: &str, name: &str) -> alloc::string::String {
        if parent_path == "/" {
            alloc::format!("/{}", name)
        } else {
            alloc::format!("{}/{}", parent_path, name)
        }
    }

    fn dirent_name_eq(raw_name: &[u8], name_len: usize, expected: &str) -> bool {
        if name_len > raw_name.len() {
            return false;
        }
        raw_name[..name_len] == *expected.as_bytes()
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
            _ => Errno::EIO,
        }
    }

    fn file_link(old_path: &str, hardlink_path: &str) -> SysResult {
        let _guard = EXT4_OP_LOCK.lock();
        let old_path = CString::new(old_path).map_err(|_| Errno::EINVAL)?;
        let new_path = CString::new(hardlink_path).map_err(|_| Errno::EINVAL)?;
        let ret = unsafe { bindings::ext4_flink(old_path.as_ptr(), new_path.as_ptr()) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        Ok(())
    }

    fn file_symlink(target: &str, path: &str) -> SysResult {
        let _guard = EXT4_OP_LOCK.lock();
        // lwext4 负责选择 fast symlink 或普通数据块存储；VFS 层只传入目标字符串和新路径。
        let target = CString::new(target).map_err(|_| Errno::EINVAL)?;
        let path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        let ret = unsafe { bindings::ext4_fsymlink(target.as_ptr(), path.as_ptr()) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        Ok(())
    }

    fn file_readlink(path: &str) -> SysResult<String> {
        const MAX_LINK_TARGET: usize = 4096;

        let _guard = EXT4_OP_LOCK.lock();
        let path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        // ext4_readlink 返回裸字节和读取长度，不保证 C 字符串结尾，因此按 rcnt 截断。
        let mut buf = Vec::from([0u8; MAX_LINK_TARGET]);
        let mut read_len = 0usize;
        let ret = unsafe {
            bindings::ext4_readlink(
                path.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut read_len,
            )
        };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        buf.truncate(read_len);
        String::from_utf8(buf).map_err(|_| Errno::EINVAL)
    }

    pub(crate) fn file_rename(old_path: &str, new_path: &str) -> SysResult {
        let _guard = EXT4_OP_LOCK.lock();
        let c_old = CString::new(old_path).map_err(|_| Errno::EINVAL)?;
        let c_new = CString::new(new_path).map_err(|_| Errno::EINVAL)?;
        let ret = unsafe { bindings::ext4_frename(c_old.as_ptr(), c_new.as_ptr()) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        Ok(())
    }

    fn file_size(&self, path: &str) -> SysResult<usize> {
        let _guard = EXT4_OP_LOCK.lock();
        let file = &mut Ext4File::new(path, self.ty.clone());
        file.file_open(path, bindings::O_RDONLY)
            .map_err(Self::map_lwext4_err)?;
        let size = file.file_size() as usize;
        file.file_close().map_err(Self::map_lwext4_err)?;
        Ok(size)
    }

    fn dirent64_reclen(name_len: usize) -> usize {
        // 目录项固定字段大小
        const DIRENT64_HEADER_SIZE: usize = 8 + 8 + 2 + 1;
        // 变长文件名字段大小
        ((DIRENT64_HEADER_SIZE + name_len + 1) + 7) & !7 // 对齐 8 字节
    }

    fn lookup_dirent(parent_path: &str, name: &str) -> SysResult<(u64, Ext4InodeTypes)> {
        let _guard = EXT4_OP_LOCK.lock();
        let c_path = CString::new(parent_path).map_err(|_| Errno::EINVAL)?;
        let c_path = c_path.into_raw();
        let mut dir: bindings::ext4_dir = unsafe { core::mem::zeroed() };
        let ret = unsafe { bindings::ext4_dir_open(&mut dir, c_path) };
        unsafe {
            drop(CString::from_raw(c_path));
        }
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }

        let mut found = None;
        loop {
            let dirent = unsafe { bindings::ext4_dir_entry_next(&mut dir) };
            if dirent.is_null() {
                break;
            }

            let dirent = unsafe { &*dirent };
            if Self::dirent_name_eq(&dirent.name, dirent.name_length as usize, name) {
                let child_path = Self::child_path(parent_path, name);
                let ty = Self::inode_mode_type(&child_path)
                    .unwrap_or_else(|| Ext4InodeTypes::from(dirent.inode_type as usize));
                found = Some((dirent.inode as u64, ty));
                break;
            }
        }

        let ret = unsafe { bindings::ext4_dir_close(&mut dir) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }

        found.ok_or(Errno::ENOENT)
    }

    fn inode_mode_type(path: &str) -> Option<Ext4InodeTypes> {
        let c_path = CString::new(path).ok()?;
        let c_path = c_path.into_raw();
        let mut mode = 0;
        let ret = unsafe { bindings::ext4_mode_get(c_path, &mut mode) };
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
        if let Some(times) = *self.times.lock() {
            return times;
        }
        InodeTimes {
            atime: Self::normalize_lower_time(atime),
            mtime: Self::normalize_lower_time(mtime),
            ctime: Self::normalize_lower_time(ctime),
        }
    }

    fn has_cached_times(&self) -> bool {
        self.times.lock().is_some()
    }

    fn write_lower_time(
        c_path: *const core::ffi::c_char,
        ts: TimeSpec,
        setter: unsafe extern "C" fn(*const core::ffi::c_char, u32) -> i32,
    ) -> SysResult {
        if ts.sec < 0 || ts.sec > u32::MAX as isize {
            return Ok(());
        }
        let ret = unsafe { setter(c_path, ts.sec as u32) };
        if ret != 0 {
            return match Self::map_lwext4_err(ret) {
                // fd 指向的文件可能已经 unlink；此时 VFS 层仍要允许 futimens/fstat
                // 作用在打开文件对象上，无法同步到底层路径时只更新 inode 缓存。
                Errno::ENOENT => Ok(()),
                err => Err(err),
            };
        }
        Ok(())
    }

    fn init_inode_times(&self) {
        let now = Self::now_timespec();
        *self.times.lock() = Some(InodeTimes {
            atime: now,
            mtime: now,
            ctime: now,
        });
    }

    fn set_cached_times(&self, times: InodeTimes) {
        *self.times.lock() = Some(times);
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
        let ty = self.node_type();
        let size = match ty {
            InodeType::Regular => match self.file_size(path) {
                Ok(size) => size.max(self.page_cache.len()),
                Err(Errno::ENOENT) if self.has_cached_times() => 0,
                Err(err) => return Err(err),
            },
            // lstat(symlink) 的 st_size 是链接目标字符串长度，不是目标文件大小。
            InodeType::SymLink => Self::file_readlink(path)?.len(),
            _ => 0,
        };
        let ino = self.ino;

        let _guard = EXT4_OP_LOCK.lock();
        let c_path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        let c_path = c_path.into_raw();

        let mut mode: u32 = 0;
        let _ = unsafe { bindings::ext4_mode_get(c_path, &mut mode) };

        let mut uid: u32 = 0;
        let mut gid: u32 = 0;
        let _ = unsafe { bindings::ext4_owner_get(c_path, &mut uid, &mut gid) };

        let mut atime: u32 = 0;
        let mut mtime: u32 = 0;
        let mut ctime: u32 = 0;
        let _ = unsafe { bindings::ext4_atime_get(c_path, &mut atime) };
        let _ = unsafe { bindings::ext4_mtime_get(c_path, &mut mtime) };
        let _ = unsafe { bindings::ext4_ctime_get(c_path, &mut ctime) };

        unsafe { drop(CString::from_raw(c_path)) };

        let times = self.cached_times(atime, mtime, ctime);

        Ok(KStat {
            dev: 0,
            size,
            ty,
            ino,
            nlink: 1,
            uid,
            gid,
            rdev: 0,
            mode,
            blksize: crate::config::BLOCK_SIZE as u32,
            blocks: KStat::blocks_for_size(size as u64),
            atime: times.atime,
            mtime: times.mtime,
            ctime: times.ctime,
        })
    }

    fn read_at(&self, path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;

        let _guard = EXT4_OP_LOCK.lock();
        let file = &mut Ext4File::new(path, self.ty.clone());
        file.file_open(path, bindings::O_RDONLY)
            .map_err(Self::map_lwext4_err)?;
        file.file_seek(off as i64, bindings::SEEK_SET)
            .map_err(Self::map_lwext4_err)?;
        let read_size = file.file_read(buf).map_err(Self::map_lwext4_err)?;
        file.file_close().map_err(Self::map_lwext4_err)?;

        Ok(read_size)
    }

    fn write_at(&self, path: &str, off: usize, buf: &[u8]) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;

        let write_size = {
            let _guard = EXT4_OP_LOCK.lock();
            let file = &mut Ext4File::new(path, self.ty.clone());
            file.file_open(path, bindings::O_RDWR)
                .map_err(Self::map_lwext4_err)?;
            file.file_seek(off as i64, bindings::SEEK_SET)
                .map_err(Self::map_lwext4_err)?;
            let write_size = file.file_write(buf).map_err(Self::map_lwext4_err)?;
            file.file_close().map_err(Self::map_lwext4_err)?;
            write_size
        };

        let now = Self::now_timespec();
        let _ = self.set_times(path, None, Some(now));

        Ok(write_size)
    }

    fn truncate(&self, path: &str, size: usize) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;

        {
            let _guard = EXT4_OP_LOCK.lock();
            let file = &mut Ext4File::new(path, self.ty.clone());
            file.file_open(path, bindings::O_RDWR)
                .map_err(Self::map_lwext4_err)?;
            file.file_truncate(size as u64)
                .map_err(Self::map_lwext4_err)?;
            file.file_close().map_err(Self::map_lwext4_err)?;
        }

        let now = Self::now_timespec();
        let _ = self.set_times(path, None, Some(now));

        Ok(0)
    }

    fn set_times(&self, path: &str, atime: Option<TimeSpec>, mtime: Option<TimeSpec>) -> SysResult {
        let mut times = if let Some(times) = *self.times.lock() {
            times
        } else {
            self.stat(path)
                .map(|stat| InodeTimes {
                    atime: stat.atime,
                    mtime: stat.mtime,
                    ctime: stat.ctime,
                })
                .unwrap_or_else(|_| {
                    let now = Self::now_timespec();
                    InodeTimes {
                        atime: now,
                        mtime: now,
                        ctime: now,
                    }
                })
        };

        let _guard = EXT4_OP_LOCK.lock();
        let c_path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        let c_path = c_path.as_ptr();

        if let Some(atime) = atime {
            Self::write_lower_time(c_path, atime, bindings::ext4_atime_set)?;
            times.atime = atime;
        }
        if let Some(mtime) = mtime {
            Self::write_lower_time(c_path, mtime, bindings::ext4_mtime_set)?;
            times.mtime = mtime;
        }

        times.ctime = Self::now_timespec();
        Self::write_lower_time(c_path, times.ctime, bindings::ext4_ctime_set)?;
        self.set_cached_times(times);
        Ok(())
    }

    fn set_mode(&self, path: &str, mode: u32) -> SysResult {
        let _guard = EXT4_OP_LOCK.lock();
        let c_path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        let ret = unsafe { bindings::ext4_mode_set(c_path.as_ptr(), mode & 0o7777) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }

        if let Some(mut times) = *self.times.lock() {
            times.ctime = Self::now_timespec();
            Self::write_lower_time(c_path.as_ptr(), times.ctime, bindings::ext4_ctime_set)?;
            self.set_cached_times(times);
        }
        Ok(())
    }

    /// 查找与 name 匹配的子索引节点，约定 name 为常规文件名
    fn lookup(&self, parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;

        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(Errno::EINVAL);
        }

        let (child_ino, child_ty) = Self::lookup_dirent(parent_path, name)?;
        Ok(Self::get_or_create(child_ino, child_ty))
    }

    fn readdir(&self, path: &str) -> SysResult<Vec<LinuxDirent64>> {
        self.check_type(InodeType::Directory)?;

        let _guard = EXT4_OP_LOCK.lock();
        let c_path = CString::new(path).map_err(|_| Errno::EINVAL)?;
        let c_path = c_path.into_raw();
        let mut dir: bindings::ext4_dir = unsafe { core::mem::zeroed() };
        let ret = unsafe { bindings::ext4_dir_open(&mut dir, c_path) };
        unsafe {
            drop(CString::from_raw(c_path));
        }
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }

        let mut entries = Vec::new();
        let mut next_off = 0usize;

        loop {
            let dirent = unsafe { bindings::ext4_dir_entry_next(&mut dir) };
            if dirent.is_null() {
                break;
            }

            let dirent = unsafe { &*dirent };
            let name_len = dirent.name_length as usize;
            let reclen = Self::dirent64_reclen(name_len);
            next_off += reclen;

            let mut d_name = dirent.name[..name_len].to_vec();
            d_name.push(0);
            entries.push(LinuxDirent64 {
                d_ino: dirent.inode as u64,
                d_off: next_off as i64,
                d_reclen: reclen as u16,
                d_type: InodeType::from(Ext4InodeTypes::from(dirent.inode_type as usize)) as u8,
                d_name,
            });
        }

        let ret = unsafe { bindings::ext4_dir_close(&mut dir) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }

        Ok(entries)
    }

    fn create(&self, parent_path: &str, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;

        let path = Self::child_path(parent_path, name);
        let ext4_ty = Ext4InodeTypes::from(ty);
        {
            let _guard = EXT4_OP_LOCK.lock();
            let file = &mut Ext4File::new(parent_path, self.ty.clone());

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
                _ => return Err(Errno::ENOSYS),
            }
        }

        let inode = self.lookup(parent_path, name)?;
        if let Some(inode) = inode.as_any().downcast_ref::<Ext4Inode>() {
            inode.init_inode_times();
        }
        Ok(inode)
    }

    fn symlink(&self, target: &str, parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;

        let path = Self::child_path(parent_path, name);
        Self::file_symlink(target, &path)?;
        // 创建后重新 lookup，复用现有 inode cache/type 修正逻辑。
        let inode = self.lookup(parent_path, name)?;
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
        Ok(())
    }

    fn unlink(&self, valid_dentry: &Arc<Dentry>) -> SysResult {
        // 调用者保证参数合法
        // self.check_type(InodeType::Directory)?;

        info!("[kernel] unlink: {}", valid_dentry.abs_path);

        let child_abs_path = &valid_dentry.abs_path;
        let child_inode = valid_dentry.try_get_inode().ok_or(Errno::ENOENT)?;
        if child_inode.node_type() == InodeType::Directory {
            let entries = child_inode.readdir(child_abs_path)?;
            let has_content = entries
                .iter()
                .any(|e| e.d_name != b".\0" && e.d_name != b"..\0");
            if has_content {
                return Err(Errno::ENOTEMPTY);
            }
            let _guard = EXT4_OP_LOCK.lock();
            let file = &mut Ext4File::new(child_abs_path, self.ty.clone());
            file.dir_rm(child_abs_path).map_err(Self::map_lwext4_err)?;
        } else {
            // lwext4_rust 包中 `file_remove` 的语义是 unlink 而非删除文件
            let _guard = EXT4_OP_LOCK.lock();
            let file = &mut Ext4File::new(child_abs_path, child_inode.node_type().into());
            file.file_remove(child_abs_path)
                .map_err(Self::map_lwext4_err)?;
        };
        Ok(())
    }

    fn read_link(&self, path: &str) -> SysResult<String> {
        // readlinkat 必须作用在 symlink inode 自身，传到这里的 path 不应已经被 namei 跟随。
        self.check_type(InodeType::SymLink)?;
        Self::file_readlink(path)
    }
}

/// 清除缓存中已死亡的 Weak 条目
fn evict_dead_inodes(cache: &mut HashMap<u64, Weak<dyn InodeOp>>) {
    let dead: Vec<u64> = cache
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
