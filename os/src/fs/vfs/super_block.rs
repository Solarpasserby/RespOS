// os/src/vfs/super_block.rs

use super::InodeOp;
use crate::fs::Statfs64;
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;

pub trait SuperBlockOp: Send + Sync {
    /// 获取根节点
    fn root_inode(&self) -> Arc<dyn InodeOp>;

    /// 将数据写回磁盘
    fn sync(&self) -> SysResult;

    /// Commit mount-wide lazy timestamp state before the lower durability
    /// barrier. Filesystems without lazy metadata need no extra work.
    fn flush_lazy_metadata(&self) -> SysResult {
        Ok(())
    }

    fn statfs(&self) -> SysResult<Statfs64> {
        Err(Errno::EINVAL)
    }
}
