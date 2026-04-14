// os/src/ext4/inode.rs

use lwext4_rust::{Ext4File, InodeTypes as Ext4InodeTypes};
use alloc::sync::Arc;
use core::cell::SyncUnsafeCell;
use core::any::Any;
use super::super::InodeOp;

pub struct Ext4Inode(SyncUnsafeCell<Ext4File>);

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode {
    pub fn new(path: &str, types: Ext4InodeTypes) -> Self {
        Ext4Inode(SyncUnsafeCell::new(Ext4File::new(path, types)))
    }
}

impl InodeOp for Ext4Inode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn create(&self, name: &str, type_: crate::fs::vfs::InodeTypes) -> Arc<dyn InodeOp> {
        
    }
}
