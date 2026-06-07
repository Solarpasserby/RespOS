// os/src/ext4/super_block.rs

use super::Ext4Inode;
use crate::drivers::Disk;
use crate::fs::Statfs64;
use crate::fs::vfs::{InodeOp, SuperBlockOp};
use crate::syscall::{Errno, SysResult};
use alloc::ffi::CString;
use alloc::sync::Arc;
use core::ffi::c_char;
use lwext4_rust::{Ext4BlockWrapper, InodeTypes as Ext4InodeTypes, bindings};
use spin::Mutex;

unsafe impl Send for Ext4SuperBlock {}
unsafe impl Sync for Ext4SuperBlock {}

pub struct Ext4SuperBlock {
    inner: Mutex<Option<Ext4BlockWrapper<Disk>>>,
    root: Arc<dyn InodeOp>,
}

impl Ext4SuperBlock {
    pub fn new(disk: Disk) -> Self {
        println!("init ext4 device superblock");
        let inner =
            Ext4BlockWrapper::<Disk>::new(disk).expect("failed to initialize EXT4 filesystem");
        let root = Ext4Inode::get_or_create(2, Ext4InodeTypes::EXT4_DE_DIR);
        Self {
            inner: Mutex::new(Some(inner)),
            root,
        }
    }

    pub fn shutdown(&self) {
        let mut inner = self.inner.lock();
        if inner.is_some() {
            if let Ok(path) = CString::new("/") {
                unsafe {
                    bindings::ext4_cache_flush(path.as_ptr());
                }
            }
        }
        let wrapper = inner.take();
        drop(inner);
        drop(wrapper);
    }
}

impl SuperBlockOp for Ext4SuperBlock {
    fn root_inode(&self) -> Arc<dyn InodeOp> {
        self.root.clone()
    }
    fn sync(&self) {
        todo!()
    }

    fn statfs(&self) -> SysResult<Statfs64> {
        let mut stats: bindings::ext4_mount_stats = unsafe { core::mem::zeroed() };
        let mount_point = CString::new("/").map_err(|_| Errno::EINVAL)?;
        let rc = unsafe {
            bindings::ext4_mount_point_stats(mount_point.as_ptr() as *const c_char, &mut stats)
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
