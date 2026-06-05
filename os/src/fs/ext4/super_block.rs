// os/src/ext4/super_block.rs

use super::Ext4Inode;
use crate::drivers::Disk;
use crate::fs::vfs::{InodeOp, SuperBlockOp};
use alloc::ffi::CString;
use alloc::sync::Arc;
use lwext4_rust::{bindings, Ext4BlockWrapper, InodeTypes as Ext4InodeTypes};
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
        // let page_cache = Some(PageCache::new_bare());
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
    // fn fs_stat(&self) -> StatFs {
    //     StatFs::new()
    // }
    fn sync(&self) {
        let inner = self.inner.lock();
        if inner.is_some() {
            if let Ok(path) = CString::new("/") {
                unsafe {
                    bindings::ext4_cache_flush(path.as_ptr());
                }
            }
        }
    }
    // fn ls(&self) {
    //     self.inner
    //         .lwext4_dir_ls()
    //         .into_iter()
    //         .for_each(|s| println!("{}", s));
    // }
}
