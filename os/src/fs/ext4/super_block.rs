// os/src/ext4/super_block.rs

use super::{EXT4_OP_LOCK, Ext4Inode, Ext4LockClass, flush_lazytime_inodes};
use crate::drivers::Disk;
use crate::fs::Statfs64;
use crate::fs::vfs::{InodeOp, SuperBlockOp};
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use core::ffi::c_char;
use lwext4_rust::{Ext4BlockWrapper, InodeTypes as Ext4InodeTypes, bindings};
use spin::Mutex;

unsafe impl Send for Ext4SuperBlock {}
unsafe impl Sync for Ext4SuperBlock {}

pub struct Ext4SuperBlock {
    inner: Mutex<Option<Ext4BlockWrapper<Disk>>>,
    root: Arc<dyn InodeOp>,
    mount_point: &'static [u8],
    fs_id: usize,
}

impl Ext4SuperBlock {
    pub fn new(
        disk: Disk,
        fs_id: usize,
        device_name: &'static str,
        mount_path: &'static str,
        mount_point: &'static [u8],
    ) -> SysResult<Self> {
        info!("initializing ext4 device {} at {}", device_name, mount_path);
        let inner = Ext4BlockWrapper::<Disk>::new_named(disk, device_name, mount_path)
            .map_err(|_| Errno::EINVAL)?;
        let root = Ext4Inode::get_or_create(
            fs_id,
            mount_path,
            mount_point,
            2,
            Ext4InodeTypes::EXT4_DE_DIR,
        );
        Ok(Self {
            inner: Mutex::new(Some(inner)),
            root,
            mount_point,
            fs_id,
        })
    }

    fn flush_cache(&self) -> SysResult {
        crate::perf::filesystem_flush(1);
        // Use the same lock as inode operations.  `inner` only protects the
        // Rust wrapper's lifetime; it does not serialize lwext4's global
        // mount/block-cache state against create/write/rename on other CPUs.
        let op_guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Superblock);
        let inner = self.inner.lock();
        if inner.is_none() {
            return Err(Errno::EIO);
        }
        let rc = {
            let _lower = op_guard.profile_lower();
            unsafe { bindings::ext4_cache_flush(self.mount_point.as_ptr().cast()) }
        };
        drop(inner);
        if rc == 0 { Ok(()) } else { Err(Errno::EIO) }
    }

    pub fn shutdown(&self) -> SysResult {
        super::reap_deferred_inodes();
        flush_lazytime_inodes(self.fs_id)?;
        self.flush_cache()?;
        let op_guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Superblock);
        let mut inner = self.inner.lock();
        let mut wrapper = inner.take();
        drop(inner);
        if let Some(wrapper) = wrapper.as_mut() {
            let _lower = op_guard.profile_lower();
            wrapper.shutdown().map_err(|_| Errno::EIO)?;
        }
        drop(wrapper);
        Ok(())
    }
}

impl SuperBlockOp for Ext4SuperBlock {
    fn root_inode(&self) -> Arc<dyn InodeOp> {
        self.root.clone()
    }
    fn sync(&self) -> SysResult {
        self.flush_cache()
    }

    fn flush_lazy_metadata(&self) -> SysResult {
        flush_lazytime_inodes(self.fs_id)
    }

    fn statfs(&self) -> SysResult<Statfs64> {
        let op_guard = EXT4_OP_LOCK.lock_class(Ext4LockClass::Superblock);
        let mut stats: bindings::ext4_mount_stats = unsafe { core::mem::zeroed() };
        let rc = {
            let _lower = op_guard.profile_lower();
            unsafe {
                bindings::ext4_mount_point_stats(
                    self.mount_point.as_ptr() as *const c_char,
                    &mut stats,
                )
            }
        };
        if rc != 0 {
            return Err(Errno::EIO);
        }
        Ok(Statfs64 {
            f_type: 0xEF53, // EXT4_SUPER_MAGIC
            f_bsize: stats.block_size as i64,
            f_blocks: stats.blocks_count,
            f_bfree: stats.free_blocks_count,
            f_bavail: stats.free_blocks_count,
            f_files: stats.inodes_count as u64,
            f_ffree: stats.free_inodes_count as u64,
            f_namelen: 255, // EXT4_NAME_LEN
            f_frsize: stats.block_size as i64,
            ..Default::default()
        })
    }
}
