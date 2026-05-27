// os/src/fs/vfs/dentry.rs

use super::InodeOp;
#[cfg(target_arch = "riscv64")]
use crate::fs::ext4::root_inode;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use hashbrown::HashMap;
use lazy_static::lazy_static;
use spin::Mutex;

/// LA64 暂不支持 ext4，提供最小根 inode 桩
#[cfg(target_arch = "loongarch64")]
fn root_inode() -> Arc<dyn InodeOp> {
    use super::InodeType;
    use crate::fs::KStat;
    use crate::syscall::{Errno, SysResult};
    use alloc::vec::Vec;

    struct StubInode;
    impl InodeOp for StubInode {
        fn as_any(&self) -> &dyn core::any::Any { self }
        fn node_type(&self) -> InodeType { InodeType::Directory }
        fn stat(&self) -> SysResult<KStat> { Err(Errno::ENOSYS) }
        fn read_at(&self, _off: usize, _buf: &mut [u8]) -> SysResult<usize> { Err(Errno::ENOSYS) }
        fn write_at(&self, _off: usize, _buf: &[u8]) -> SysResult<usize> { Err(Errno::ENOSYS) }
        fn truncate(&self, _size: usize) -> SysResult<usize> { Err(Errno::ENOSYS) }
        fn lookup(&self, _name: &str) -> SysResult<Arc<dyn InodeOp>> { Err(Errno::ENOSYS) }
        fn readdir(&self) -> SysResult<Vec<LinuxDirent64>> { Err(Errno::ENOSYS) }
        fn create(&self, _name: &str, _ty: InodeType) -> SysResult<Arc<dyn InodeOp>> { Err(Errno::ENOSYS) }
        fn link(&self, _bare_dentry: Arc<Dentry>) -> SysResult { Err(Errno::ENOSYS) }
        fn unlink(&self, _valid_dentry: Arc<Dentry>) -> SysResult { Err(Errno::ENOSYS) }
    }
    Arc::new(StubInode)
}

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
        self.try_get_inode()
            .expect("[kernel] func:get_inode() the inode is negative!")
    }
    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        self.inner.lock().parent.clone()
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

    /// 是否为根目录项
    pub fn is_root(&self) -> bool {
        self.abs_path == "/" && self.get_parent().is_none()
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
    pub fn new(parent_dentry: Option<Arc<Dentry>>, inode: Arc<dyn InodeOp>) -> Self {
        Self {
            inode: Some(inode),
            parent: parent_dentry,
            children: HashMap::new(),
        }
    }
    // 负目录项
    pub fn negative(parent_dentry: Option<Arc<Dentry>>) -> Self {
        Self {
            inode: None,
            parent: parent_dentry,
            children: HashMap::new(),
        }
    }
}

// 内核内部使用的目录项描述，真正返回给用户时需要按 linux_dirent64 的变长布局序列化成字节
#[derive(Clone, Debug)]
pub struct LinuxDirent64 {
    pub d_ino: u64,      // inode 号
    pub d_off: i64,      // 下一个目录项偏移
    pub d_reclen: u16,   // 当前目录项记录长度
    pub d_type: u8,      // 文件类型
    pub d_name: Vec<u8>, // 文件名，0 结尾，变长
}

impl LinuxDirent64 {
    pub fn copy_to_buffer(&self, buf: &mut [u8]) {
        buf[0..8].copy_from_slice(&self.d_ino.to_le_bytes());
        buf[8..16].copy_from_slice(&self.d_off.to_le_bytes());
        buf[16..18].copy_from_slice(&self.d_reclen.to_le_bytes());
        buf[18] = self.d_type;
        let name_len = self.d_name.len();
        buf[19..19 + name_len].copy_from_slice(&self.d_name[..]);
    }
}
