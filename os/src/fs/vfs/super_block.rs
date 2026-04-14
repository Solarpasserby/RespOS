// os/src/vfs/super_block.rs

use alloc::sync::Arc;
use super::InodeOp;

pub trait SuperBlockOp: Send + Sync {
    /// 获取根节点
    fn root_inode(&self) -> Arc<dyn InodeOp>;

    /// 将数据写回磁盘
    fn sync(&self);

    // // 显示文件系统的信息
    // fn fs_stat(&self) -> StatFs;
    // /// 列出应用
    // fn ls(&self);
}
