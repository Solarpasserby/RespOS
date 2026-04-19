use alloc::sync::{Arc, Weak};
use alloc::string::String;
use spin::Mutex;
use hashbrown::HashMap;
use super::{InodeOp, InodeType};

pub struct Dentry {
    pub name: String,
    pub parent: Option<Weak<Dentry>>,
    pub inode: Mutex<Option<Arc<dyn InodeOp>>>,
    pub children: Mutex<HashMap<String, Arc<Dentry>>>,
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub ty: InodeType,
}
