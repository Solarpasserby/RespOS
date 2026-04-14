// os/src/ext4/inode.rs

use lwext4_rust::{Ext4File, InodeTypes as Ext4InodeTypes, bindings};
use alloc::sync::Arc;
use core::cell::SyncUnsafeCell;
use core::any::Any;
use super::super::{InodeOp, InodeTypes};

pub struct Ext4Inode(SyncUnsafeCell<Ext4File>);

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode {
    pub fn new(path: &str, type_: Ext4InodeTypes) -> Self {
        Ext4Inode(SyncUnsafeCell::new(Ext4File::new(path, type_)))
    }
}

impl InodeOp for Ext4Inode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn size(&self) -> usize {
        let file = unsafe { &mut *self.0.get() };
        let type_ = InodeTypes::from(file.file_type_get());
        if type_ == InodeTypes::File {
            let path_buf = file.get_path();
            let path = path_buf.to_str().unwrap();
            let _ = file.file_open(path, bindings::O_RDONLY);
            let file_size = file.file_size();
            let _ = file.file_close();
            file_size as usize
        } else {
            0
        }
    }

    fn node_type(&self) -> InodeTypes {
        let file = unsafe { &mut *self.0.get() };
        InodeTypes::from(file.file_type_get())
    }

    fn read_at(&self, off: usize, buf: &mut [u8]) -> usize {
        let file = unsafe { &mut *self.0.get() };

        let path_buf = file.get_path();
        let path = path_buf.to_str().unwrap();

        file.file_open(path, bindings::O_RDONLY).unwrap();
        file.file_seek(off as i64, bindings::SEEK_SET).unwrap();

        let read_size = file.file_read(buf).unwrap();

        let _ = file.file_close();
        read_size
    }

    fn write_at(&self, off: usize, buf: &[u8]) -> usize {
        let file = unsafe { &mut *self.0.get() };

        let path_buf = file.get_path();
        let path = path_buf.to_str().unwrap();

        file.file_open(path, bindings::O_RDWR).unwrap();
        file.file_seek(off as i64, bindings::SEEK_SET).unwrap();

        let write_size = file.file_write(buf).unwrap();

        let _ = file.file_close();
        write_size
    }

    fn create(&self, path: &str, type_: InodeTypes) -> Option<Arc<dyn InodeOp>> {
        let type_ = Ext4InodeTypes::from(type_);
        let file = unsafe { &mut *self.0.get() };
        let new_inode = Ext4Inode::new(path, type_.clone());

        if !file.check_inode_exist(path, type_.clone()) {
            let new_file = unsafe { &mut *new_inode.0.get() };
            if type_ == Ext4InodeTypes::EXT4_DE_DIR
                && new_file.dir_mk(path).is_err() {
                return None;
            }
            if new_file.file_open(path, bindings::O_RDWR | bindings::O_CREAT | bindings::O_TRUNC).is_err() {
                return None;
            }
            new_file.file_close();
        }

        Some(Arc::new(new_inode))
    }

    fn lookup(&self, path: &str) -> Arc<dyn InodeOp> {
        let file = unsafe { &mut *self.0.get() };

        if file.check_inode_exist(path, Ext4InodeTypes::EXT4_DE_DIR) {
            Arc::new(Ext4Inode::new(path, Ext4InodeTypes::EXT4_DE_DIR))
        } else if file.check_inode_exist(path, Ext4InodeTypes::EXT4_DE_REG_FILE) {
            Arc::new(Ext4Inode::new(path, Ext4InodeTypes::EXT4_DE_REG_FILE))
        } else {
            panic!("inode not found: {path}");
        }
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let file = unsafe { &mut *self.0.get() };
        file.file_close().expect("[kernel] CRATE lwext4_rust: Failed to close file.");
    }
}

impl From<InodeTypes> for Ext4InodeTypes {
    fn from(types: InodeTypes) -> Self {
        match types {
            InodeTypes::BlockDevice => Ext4InodeTypes::EXT4_DE_BLKDEV,
            InodeTypes::CharDevice => Ext4InodeTypes::EXT4_DE_CHRDEV,
            InodeTypes::Dir => Ext4InodeTypes::EXT4_DE_DIR,
            InodeTypes::Fifo => Ext4InodeTypes::EXT4_DE_FIFO,
            InodeTypes::File => Ext4InodeTypes::EXT4_DE_REG_FILE,
            InodeTypes::Socket => Ext4InodeTypes::EXT4_DE_SOCK,
            InodeTypes::SymLink => Ext4InodeTypes::EXT4_DE_SYMLINK,
            InodeTypes::Unknown => Ext4InodeTypes::EXT4_DE_UNKNOWN,
        }
    }
}

impl From<Ext4InodeTypes> for InodeTypes {
    fn from(types: Ext4InodeTypes) -> Self {
        match types {
            Ext4InodeTypes::EXT4_INODE_MODE_FIFO => InodeTypes::Fifo,
            Ext4InodeTypes::EXT4_INODE_MODE_CHARDEV => InodeTypes::CharDevice,
            Ext4InodeTypes::EXT4_INODE_MODE_DIRECTORY => InodeTypes::Dir,
            Ext4InodeTypes::EXT4_INODE_MODE_BLOCKDEV => InodeTypes::BlockDevice,
            Ext4InodeTypes::EXT4_INODE_MODE_FILE => InodeTypes::File,
            Ext4InodeTypes::EXT4_INODE_MODE_SOFTLINK => InodeTypes::SymLink,
            Ext4InodeTypes::EXT4_INODE_MODE_SOCKET => InodeTypes::Socket,
            _ => panic!("[kernel] Unavailable Ext4 Inode Type!"),
        }
    }
}
