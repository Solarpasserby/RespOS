// os/src/fs/vfs/dentry.rs

use lazy_static::lazy_static;
use alloc::sync::{Arc, Weak};
use alloc::string::String;
use spin::Mutex;
use hashbrown::HashMap;
use crate::fs::ext4::root_inode;
use super::{InodeOp, InodeType};

lazy_static! {
    pub static ref ROOT_DENTRY: Arc<Dentry> = {
        let inode = root_inode();
        Arc::new(Dentry::new("/".into(), None, inode))
    };
}


pub struct Dentry {
    pub abs_path: String,
    pub inner: Mutex<DentryInner>,
}

impl Dentry {
    pub fn get_inode(&self) -> Option<Arc<dyn InodeOp>> {
        self.inner.lock().inode.clone()
    }
    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        self.inner
            .lock()
            .parent
            .clone()
            .map(|p| p.upgrade().unwrap())
    }
    pub fn set_parent(&self, parent: Arc<Dentry>) {
        self.inner.lock().parent = Some(Arc::downgrade(&parent));
        // self.inner.lock().parent = Some(parent);
    }
    pub fn get_child(self: &Arc<Dentry>, name: &str) -> Option<Arc<Dentry>> {
        let inner = self.inner.lock();
        if let Some(child) = inner.children.get(name) {
            return Some(child.clone());
        }
        None
    }

    /// 用于处理路径解析时根目录没有父目录的问题
    /// 
    /// 普通目录返回其父目录，根目录返回自身
    pub fn get_parent_or_self(self: &Arc<Self>) -> Arc<Dentry> {
        self.get_parent().unwrap_or_else(|| self.clone())
    }
}

impl Dentry {
    pub fn zero_init() -> Self {
        Self {
            abs_path: String::new(),
            inner: Mutex::new(DentryInner::negative(None)),
        }
    }
    pub fn new(abs_path: String, parent: Option<Arc<Dentry>>, inode: Arc<dyn InodeOp>) -> Self {
        Self {
            abs_path,
            inner: Mutex::new(DentryInner::new(parent, inode)),
        }
    }
    pub fn negative(abs_path: String, parent: Option<Arc<Dentry>>) -> Arc<Self> {
        Arc::new(Self {
            abs_path,
            inner: Mutex::new(DentryInner::negative(parent)),
        })
    }
}

pub struct DentryInner {
    pub inode: Option<Arc<dyn InodeOp>>,
    pub parent: Option<Weak<Dentry>>,
    pub children: HashMap<String, Arc<Dentry>>,
}

impl DentryInner {
    pub fn new(parent: Option<Arc<Dentry>>, inode: Arc<dyn InodeOp>) -> Self {
        Self {
            inode: Some(inode),
            parent: parent.map(|p| Arc::downgrade(&p)),
            children: HashMap::new(),
        }
    }
    // 负目录项
    pub fn negative(parent: Option<Arc<Dentry>>) -> Self {
        Self {
            inode: None,
            parent: parent.map(|p| Arc::downgrade(&p)),
            children: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub ty: InodeType,
}
