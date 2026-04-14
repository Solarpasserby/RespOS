// os/src/vfs/dentry.rs

use spin::Mutex;
use hashbrown::HashMap;
use alloc::sync::{Arc, Weak};
use alloc::string::String;
use super::InodeOp;

// VFS层的统一目录项结构
#[repr(C)]
pub struct Dentry {
    pub absolute_path: String,
    pub inner: Mutex<DentryInner>,
}

pub struct DentryInner {
    // None 表示该 dentry 未关联 inode
    pub inode: Option<Arc<dyn InodeOp>>,
    // pub inode: Option<Arc<SpinNoIrqLock<OSInode>>>,
    pub parent: Option<Weak<Dentry>>,
    // chrildren 是一个哈希表, 用于存储子目录/文件, name不是绝对路径
    pub children: HashMap<String, Arc<Dentry>>,
}
