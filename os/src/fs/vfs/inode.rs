// os/src/vfs/inode.rs

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use crate::syscall::SysResult;
use crate::fs::KStat;
use super::LinuxDirent64;

pub trait InodeOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn node_type(&self) -> InodeType;
    fn stat(&self) -> SysResult<KStat>;

    fn read_at(&self, off: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn write_at(&self, off: usize, buf: &[u8]) -> SysResult<usize>;
    fn truncate(&self, size: usize) -> SysResult<usize>;

    fn lookup(&self, name: &str) -> SysResult<Arc<dyn InodeOp>>;
    fn readdir(&self) -> SysResult<Vec<LinuxDirent64>>;

    fn create(&self, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>>;
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
/// 文件类型
pub enum InodeType {
    /// 无效
    Unknown     = 0o0,
    /// FIFO 管道
    Fifo        = 0o1,
    /// 字符设备
    CharDevice  = 0o2,
    /// 目录
    Directory   = 0o4,
    /// 块设备
    BlockDevice = 0o6,
    /// 常规文件
    Regular     = 0o10,
    /// 符号链接文件
    SymLink     = 0o12,
    /// 套接字
    Socket      = 0o14,
}
