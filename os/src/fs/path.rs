// os/src/fs/path.rs

use super::mount::{VfsMount, path_global_abs_path};
use super::vfs::Dentry;
use alloc::string::String;
use alloc::sync::Arc;

/// 路径 = (挂载的文件系统, 文件系统内的目录项)
///
/// 类比 Linux 的 `struct path { vfsmount *mnt; dentry *dentry; }`。
pub struct Path {
    pub mnt: Arc<VfsMount>,
    pub dentry: Arc<Dentry>,
}

impl Path {
    pub fn new(mnt: Arc<VfsMount>, dentry: Arc<Dentry>) -> Arc<Self> {
        Arc::new(Path { mnt, dentry })
    }

    /// 占位路径，仅用于初始化前的零值填充
    pub fn zero_init() -> Arc<Self> {
        Arc::new(Path {
            mnt: VfsMount::zero_init(),
            dentry: Arc::new(Dentry::zero_init()),
        })
    }

    pub fn from_existed_user(path: &Arc<Path>) -> Arc<Self> {
        Arc::new(Path {
            mnt: path.mnt.clone(),
            dentry: path.dentry.clone(),
        })
    }

    /// 当前路径在所属文件系统中的绝对路径
    pub fn abs_path(&self) -> String {
        self.dentry.abs_path.clone()
    }

    pub fn global_abs_path(&self) -> String {
        path_global_abs_path(self)
    }
}
