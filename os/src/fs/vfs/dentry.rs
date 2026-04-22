// os/src/fs/vfs/dentry.rs

use lazy_static::lazy_static;
use alloc::sync::{Arc, Weak};
use alloc::string::{String, ToString};
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
    // 获取内部数据
    pub fn try_get_inode(&self) -> Option<Arc<dyn InodeOp>> {
        self.inner.lock().inode.clone()
    }
    pub fn get_inode(&self) -> Arc<dyn InodeOp> {
        self.try_get_inode().expect("[kernel] func:get_inode() the inode is negative!")
    }
    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        self.inner
            .lock()
            .parent
            .clone()
    }
    pub fn set_parent(&self, parent: Arc<Dentry>) {
        self.inner.lock().parent = Some(parent);
        // self.inner.lock().parent = Some(parent);
    }
    pub fn get_child(self: &Arc<Dentry>, name: &str) -> Option<Arc<Dentry>> {
        let mut inner = self.inner.lock();
        let child = match inner.children.get(name) {
            Some(child) => child.upgrade(),
            None => None,
        };

        // 当 child 为空将其移除
        if child.is_none() {
            inner.children.remove(name);
        }

        child
    }

    /// 插入孩子，仅更新自身状态，孩子的父亲需自己设置（好奇怪的描述）
    pub fn insert_child(self: &Arc<Self>, name: &str, child: Arc<Dentry>) {
        // child.set_parent(self.clone());
        self.inner
            .lock()
            .children
            .insert(name.to_string(), Arc::downgrade(&child));
    }
    /// 删除孩子，仅更新自身状态
    pub fn remove_child(&self, name: &str) {
        self.inner.lock().children.remove(name);
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

/// 目录项内部数据
/// 
/// 将 parent 变为强引用，将 child 变为弱引用
/// 主要是考虑到前者难以查找，而后者可以查找
pub struct DentryInner {
    pub inode: Option<Arc<dyn InodeOp>>,
    pub parent: Option<Arc<Dentry>>,
    pub children: HashMap<String, Weak<Dentry>>,
}

impl DentryInner {
    pub fn new(parent: Option<Arc<Dentry>>, inode: Arc<dyn InodeOp>) -> Self {
        Self {
            inode: Some(inode),
            parent: parent,
            children: HashMap::new(),
        }
    }
    // 负目录项
    pub fn negative(parent: Option<Arc<Dentry>>) -> Self {
        Self {
            inode: None,
            parent: parent,
            children: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub ty: InodeType,
}
