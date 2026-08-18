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
    static ref SUPER_BLOCK: Option<Arc<Ext4SuperBlock>> = {
        match BlockDeviceImpl::new_device(0) {
            Ok(device) => Some(Arc::new(
                Ext4SuperBlock::new(
                Disk::new(Arc::new(device), crate::config::ROOT_DISK_BASE_BLOCK),
                0,
                "ext4_root",
                "/",
                b"/\0",
            )
                .expect("[kernel] failed to initialize root EXT4 filesystem"),
            )),
            // 无块设备（真机 Stage 4 未接 AHCI / QEMU 未挂盘）：根 ext4 不可用，
            // 由 init_root_fs 降级为零初始化根路径，让无盘也能进用户态。
            Err(_) => None,
        }
    };
    static ref AUXILIARY_SUPER_BLOCK: Mutex<Option<Arc<Ext4SuperBlock>>> = Mutex::new(None);
}

pub fn auxiliary_super_block() -> crate::syscall::SysResult<Arc<Ext4SuperBlock>> {
    if let Some(super_block) = AUXILIARY_SUPER_BLOCK.lock().as_ref() {
        return Ok(super_block.clone());
    }
    let device = BlockDeviceImpl::new_device(1).map_err(|_| crate::syscall::Errno::ENODEV)?;
    let super_block = Arc::new(Ext4SuperBlock::new(
        Disk::new(Arc::new(device), 0),
        1,
        "ext4_aux",
        "/respos/",
        b"/respos/\0",
    )?);
    *AUXILIARY_SUPER_BLOCK.lock() = Some(super_block.clone());
    Ok(super_block)
}

pub fn root_inode() -> Arc<dyn InodeOp> {
    super_block().root_inode()
}

/// 尝试获取根 ext4 超级块；无块设备时返回 `None`。
pub fn try_super_block() -> Option<Arc<dyn SuperBlockOp>> {
    SUPER_BLOCK.clone().map(|sb| sb as Arc<dyn SuperBlockOp>)
}

/// 获取根 ext4 超级块；无块设备时 panic（供磁盘必需路径使用）。
pub fn super_block() -> Arc<dyn SuperBlockOp> {
    try_super_block().expect("[kernel] required root virtio block device is unavailable")
}

pub fn shutdown() -> crate::syscall::SysResult {
    // Flush every ext4 instance before poweroff. Keep trying the root even if
    // the auxiliary disk fails, so one device cannot prevent the other from
    // receiving its journal/cache and virtio flush barriers.
    let auxiliary_result = AUXILIARY_SUPER_BLOCK
        .lock()
        .as_ref()
        .map_or(Ok(()), |super_block| super_block.shutdown());
    let root_result = SUPER_BLOCK
        .as_ref()
        .map_or(Ok(()), |super_block| super_block.shutdown());
    auxiliary_result?;
    root_result
}
