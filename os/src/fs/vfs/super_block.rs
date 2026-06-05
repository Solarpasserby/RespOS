// os/src/vfs/super_block.rs

use super::InodeOp;
use crate::fs::Statfs64;
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;

pub trait SuperBlockOp: Send + Sync {
    /// 获取根节点
    fn root_inode(&self) -> Arc<dyn InodeOp>;

    /// 将数据写回磁盘
    fn sync(&self);

    fn statfs(&self) -> SysResult<Statfs64> {
        Err(Errno::EINVAL)
    }
}
