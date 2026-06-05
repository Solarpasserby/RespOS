// os/src/fs/proc/mod.rs

//! 虚拟 procfs 文件系统。
//!
//! 子模块划分：
//! - `dirs`  — `/proc` 和 `/proc/self` 的目录 inode
//! - `smaps` — `/proc/self/smaps` 虚拟文件，动态生成内存映射信息
//!
//! 后续可在此目录下新增 `exe`、`maps`、`stat`、`status` 等子模块。

mod dirs;
mod exe;
mod meminfo;
mod mounts;
mod smaps;
mod stat;

use super::vfs::Dentry;
use alloc::sync::Arc;
use spin::Mutex;

use dirs::{ProcDirInode, ProcSelfInode};

/// 持有 /proc 根 dentry 的强引用，防止 Weak 引用失效导致 procfs 不可达。
static PROC_ROOT: Mutex<Option<Arc<Dentry>>> = Mutex::new(None);

/// 在根文件系统中创建 /proc/self/smaps 目录树。
///
/// 调用时机：文件系统初始化完成后（`init_root_fs` 末尾）。
pub fn init_procfs(root: Arc<Dentry>) {
    let proc_dentry = Arc::new(Dentry::new(
        "/proc".into(),
        Some(root.clone()),
        Arc::new(ProcDirInode),
    ));
    root.insert_child("proc", proc_dentry.clone());

    let self_dentry = Arc::new(Dentry::new(
        "/proc/self".into(),
        Some(proc_dentry.clone()),
        Arc::new(ProcSelfInode),
    ));
    proc_dentry.insert_child("self", self_dentry);

    *PROC_ROOT.lock() = Some(proc_dentry);
}
