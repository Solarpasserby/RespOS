// os/src/vfs/inode.rs

use alloc::sync::Arc;
use core::any::Any;

pub trait InodeOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn size(&self) -> usize;
    fn node_type(&self) -> InodeTypes;
    fn read_at(&self, off: usize, buf: &mut [u8]) -> usize;
    fn write_at(&self, off: usize, buf: &[u8]) -> usize;
    fn create(&self, path: &str, type_: InodeTypes) -> Option<Arc<dyn InodeOp>>;
    fn lookup(&self, path: &str) -> Arc<dyn InodeOp>;
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum InodeTypes {
    /// 无效
    Unknown = 0o0,
    /// FIFO 管道
    Fifo = 0o1,
    /// 字符设备
    CharDevice = 0o2,
    /// 目录
    Dir = 0o4,
    /// 块设备
    BlockDevice = 0o6,
    /// 文件
    File = 0o10,
    /// 符号链接文件
    SymLink = 0o12,
    /// Socket 套接字
    Socket = 0o14,
}
