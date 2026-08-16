// os/src/vfs/inode.rs

use super::{Dentry, LinuxDirent64};
use crate::fs::page_cache::PageCache;
use crate::fs::KStat;
use crate::syscall::{Errno, SysResult};
use crate::timer::TimeSpec;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

pub trait InodeOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn node_type(&self) -> InodeType;
    fn stat(&self, path: &str) -> SysResult<KStat>;

    /// 成功返回 `n` 时必须已初始化 `buf[..n]`，且 `n <= buf.len()`。
    fn read_at(&self, path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn write_at(&self, path: &str, off: usize, buf: &[u8]) -> SysResult<usize>;
    fn truncate(&self, path: &str, size: usize) -> SysResult<usize>;

    /// Whether a read can complete without waiting. Character devices may
    /// override this so poll(2) and O_NONBLOCK observe device state.
    fn read_ready(&self) -> bool {
        true
    }

    /// Deallocate full filesystem blocks and zero partial blocks without
    /// changing the logical file size.
    fn punch_hole(&self, _path: &str, _offset: usize, _len: usize) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }

    /// 返回共享页缓存。仅 ext4 常规文件返回 Some，其余返回 None。
    fn get_page_cache(&self) -> Option<Arc<PageCache>> {
        None
    }
    /// Character devices such as /dev/zero can request anonymous mmap
    /// semantics instead of being constrained by their stat size.
    fn mmap_zero_filled(&self) -> bool {
        false
    }
    /// Publish the logical write time before delayed data writeback.  Regular
    /// files may keep this metadata pending until fsync/syncfs/unmount.
    fn note_data_write(&self, _path: &str, _time: TimeSpec) -> SysResult {
        Ok(())
    }
    fn flush_data_metadata(&self, _path: &str) -> SysResult {
        Ok(())
    }
    fn has_pending_data_metadata(&self) -> bool {
        false
    }
    fn set_times(
        &self,
        _path: &str,
        _atime: Option<TimeSpec>,
        _mtime: Option<TimeSpec>,
    ) -> SysResult {
        Err(Errno::EINVAL)
    }
    fn set_mode(&self, _path: &str, _mode: u32) -> SysResult {
        Err(Errno::EINVAL)
    }
    fn set_owner(&self, _path: &str, _uid: u32, _gid: u32) -> SysResult {
        Err(Errno::EINVAL)
    }
    fn set_owner_and_mode(
        &self,
        _path: &str,
        _uid: u32,
        _gid: u32,
        _mode: Option<u32>,
    ) -> SysResult {
        Err(Errno::EOPNOTSUPP)
    }
    fn set_xattr(&self, _name: String, _value: Vec<u8>, _flags: usize) -> SysResult {
        Err(Errno::EPERM)
    }
    fn get_xattr(&self, _name: &str) -> Result<Vec<u8>, Errno> {
        Err(Errno::ENODATA)
    }
    fn list_xattr(&self) -> Result<Vec<String>, Errno> {
        Ok(Vec::new())
    }
    fn remove_xattr(&self, _name: &str) -> SysResult {
        Err(Errno::ENODATA)
    }
    fn clear_xattrs(&self) {}

    fn lookup(&self, parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>>;
    fn readdir(&self, path: &str) -> SysResult<Vec<LinuxDirent64>>;

    fn create(&self, parent_path: &str, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>>;

    /// Create a special inode whose device payload is part of the on-disk
    /// identity. Filesystems without a real special-inode representation must
    /// reject character/block devices instead of creating regular placeholders.
    fn create_special(
        &self,
        parent_path: &str,
        name: &str,
        ty: InodeType,
        _dev: u32,
    ) -> SysResult<Arc<dyn InodeOp>> {
        match ty {
            InodeType::Fifo | InodeType::Socket => self.create(parent_path, name, ty),
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn symlink(
        &self,
        _target: &str,
        _parent_path: &str,
        _name: &str,
    ) -> SysResult<Arc<dyn InodeOp>> {
        Err(Errno::ENOSYS)
    }

    fn link(&self, old_path: &str, bare_dentry: Arc<Dentry>) -> SysResult;
    fn unlink(&self, valid_dentry: &Arc<Dentry>) -> SysResult;

    /// 读取符号链接的目标路径。仅 SymLink 类型需要实现。
    fn read_link(&self, _path: &str) -> SysResult<String> {
        Err(Errno::EINVAL)
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
/// 文件类型
pub enum InodeType {
    /// 无效
    Unknown = 0o0,
    /// FIFO 管道
    Fifo = 0o1,
    /// 字符设备
    CharDevice = 0o2,
    /// 目录
    Directory = 0o4,
    /// 块设备
    BlockDevice = 0o6,
    /// 常规文件
    Regular = 0o10,
    /// 符号链接文件
    SymLink = 0o12,
    /// 套接字
    Socket = 0o14,
}
