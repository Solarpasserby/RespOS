// os/src/vfs/inode.rs

use super::{Dentry, LinuxDirent64};
use crate::fs::KStat;
use crate::syscall::SysResult;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

pub trait InodeOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn node_type(&self) -> InodeType;
    fn stat(&self, path: &str) -> SysResult<KStat>;

    fn read_at(&self, path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn write_at(&self, path: &str, off: usize, buf: &[u8]) -> SysResult<usize>;
    fn truncate(&self, path: &str, size: usize) -> SysResult<usize>;

    fn lookup(&self, parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>>;
    fn readdir(&self, path: &str) -> SysResult<Vec<LinuxDirent64>>;

    fn create(&self, parent_path: &str, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>>;

    fn link(&self, old_path: &str, bare_dentry: Arc<Dentry>) -> SysResult;
    fn unlink(&self, valid_dentry: Arc<Dentry>) -> SysResult;
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
