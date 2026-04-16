// os/src/ext4/super_block.rs

use lwext4_rust::{Ext4BlockWrapper, InodeTypes as Ext4InodeTypes};
use alloc::sync::Arc;
use crate::drivers::Disk;
use crate::fs::vfs::{InodeOp, SuperBlockOp};
use super::Ext4Inode;

unsafe impl Send for Ext4SuperBlock {}
unsafe impl Sync for Ext4SuperBlock {}

pub struct Ext4SuperBlock {
    inner: Ext4BlockWrapper<Disk>,
    root: Arc<dyn InodeOp>,
}

impl Ext4SuperBlock {
    pub fn new(disk: Disk) -> Self {
        println!("init ext4 device superblock");
        let inner =
            Ext4BlockWrapper::<Disk>::new(disk).expect("failed to initialize EXT4 filesystem");
        // let page_cache = Some(PageCache::new_bare());
        let root = Arc::new(Ext4Inode::new("/", Ext4InodeTypes::EXT4_DE_DIR));
        Self { inner, root }
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
        todo!()
    }
    // fn ls(&self) {
    //     self.inner
    //         .lwext4_dir_ls()
    //         .into_iter()
    //         .for_each(|s| println!("{}", s));
    // }
}
