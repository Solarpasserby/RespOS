// os/src/fs/ext4/mod.rs

mod inode;
mod super_block;

use super::vfs::{InodeOp, SuperBlockOp};
use crate::drivers::{BlockDeviceImpl, Disk};
use alloc::sync::Arc;
use lazy_static::lazy_static;
use spin::Mutex;

pub use inode::*;
pub use super_block::*;

lazy_static! {
    static ref SUPER_BLOCK: Arc<Ext4SuperBlock> = {
        let device = BlockDeviceImpl::new_device(0)
            .expect("[kernel] required root virtio block device is unavailable");
        Arc::new(
            Ext4SuperBlock::new(Disk::new(Arc::new(device)), 0, "ext4_root", "/", b"/\0")
                .expect("[kernel] failed to initialize root EXT4 filesystem"),
        )
    };
    static ref AUXILIARY_SUPER_BLOCK: Mutex<Option<Arc<Ext4SuperBlock>>> = Mutex::new(None);
}

pub fn auxiliary_super_block() -> crate::syscall::SysResult<Arc<Ext4SuperBlock>> {
    if let Some(super_block) = AUXILIARY_SUPER_BLOCK.lock().as_ref() {
        return Ok(super_block.clone());
    }
    let device = BlockDeviceImpl::new_device(1).map_err(|_| crate::syscall::Errno::ENODEV)?;
    let super_block = Arc::new(Ext4SuperBlock::new(
        Disk::new(Arc::new(device)),
        1,
        "ext4_aux",
        "/respos/",
        b"/respos/\0",
    )?);
    *AUXILIARY_SUPER_BLOCK.lock() = Some(super_block.clone());
    Ok(super_block)
}

pub fn root_inode() -> Arc<dyn InodeOp> {
    SUPER_BLOCK.root_inode()
}

pub fn super_block() -> Arc<dyn SuperBlockOp> {
    SUPER_BLOCK.clone()
}

pub fn shutdown() -> crate::syscall::SysResult {
    // Flush every ext4 instance before poweroff. Keep trying the root even if
    // the auxiliary disk fails, so one device cannot prevent the other from
    // receiving its journal/cache and virtio flush barriers.
    let auxiliary_result = AUXILIARY_SUPER_BLOCK
        .lock()
        .as_ref()
        .map_or(Ok(()), |super_block| super_block.shutdown());
    let root_result = SUPER_BLOCK.shutdown();
    auxiliary_result?;
    root_result
}
