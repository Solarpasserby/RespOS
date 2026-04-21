// os/src/fs/ext4/mod.rs

mod inode;
mod super_block;

use lazy_static::lazy_static;
use alloc::sync::Arc;
use crate::drivers::{BlockDeviceImpl, Disk};
use super::vfs::{InodeOp, SuperBlockOp};

pub use inode::*;
pub use super_block::*;

lazy_static! {
    static ref SUPER_BLOCK: Arc<dyn SuperBlockOp> = {
        Arc::new(Ext4SuperBlock::new(
            Disk::new(Arc::new(BlockDeviceImpl::new_device())),
        ))
    };
}

pub fn root_inode() -> Arc<dyn InodeOp> {
    SUPER_BLOCK.root_inode()
}