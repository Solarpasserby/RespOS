// os/src/fs/proc/mod.rs

//! 虚拟 procfs 文件系统。
//!
//! 子模块划分：
//! - `dirs`  — `/proc` 和 `/proc/self` 的目录 inode
//! - `smaps` — `/proc/self/smaps` 虚拟文件，动态生成内存映射信息
//!
//! 后续可在此目录下新增 `exe`、`maps`、`stat`、`status` 等子模块。

mod cpuinfo;
mod dirs;
mod exe;
mod health;
mod maps;
mod meminfo;
mod mounts;
mod smaps;
mod stat;
mod version;

use super::Statfs64;
use super::vfs::{Dentry, InodeOp, SuperBlockOp};
use crate::fs::dentry_cache;
use crate::fs::mount::{self, Mount, VfsMount, get_mount_by_dentry};
use alloc::sync::Arc;

use dirs::{ProcDirInode, ProcSelfInode};

const PROC_SUPER_MAGIC: i64 = 0x9fa0;

struct ProcSuperBlock;

impl SuperBlockOp for ProcSuperBlock {
    fn root_inode(&self) -> Arc<dyn InodeOp> {
        Arc::new(ProcDirInode)
    }

    fn sync(&self) {}

    fn statfs(&self) -> crate::syscall::SysResult<Statfs64> {
        Ok(Statfs64 {
            f_type: PROC_SUPER_MAGIC,
            f_bsize: crate::config::PAGE_SIZE as i64,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 1024,
            f_ffree: 0,
            f_namelen: 255,
            f_frsize: crate::config::PAGE_SIZE as i64,
            ..Default::default()
        })
    }
}

/// 在根文件系统中创建 /proc 虚拟目录树。
///
/// 调用时机：文件系统初始化完成后（`init_root_fs` 末尾）。
pub fn init_procfs(root: Arc<Dentry>) {
    let proc_mountpoint = Arc::new(Dentry::new(
        "/proc".into(),
        Some(root.clone()),
        Arc::new(ProcDirInode),
    ));
    root.insert_child("proc", proc_mountpoint.clone());
    dentry_cache::insert_dentry_cache(proc_mountpoint.clone());
    dentry_cache::pin_vfs_dentry(proc_mountpoint.clone());

    let proc_root = Arc::new(Dentry::new("/".into(), None, Arc::new(ProcDirInode)));
    dentry_cache::pin_vfs_dentry(proc_root.clone());
    let proc_mount = VfsMount::new(proc_root.clone(), Arc::new(ProcSuperBlock), 0);
    let parent_mount = get_mount_by_dentry(&root).expect("[procfs] root mount is not initialized");
    mount::add_mount(Mount::new_child(proc_mountpoint, proc_mount, parent_mount));

    let self_dentry = Arc::new(Dentry::new(
        "/self".into(),
        Some(proc_root.clone()),
        Arc::new(ProcSelfInode),
    ));
    proc_root.insert_child("self", self_dentry.clone());
    dentry_cache::insert_dentry_cache(self_dentry.clone());
    dentry_cache::pin_vfs_dentry(self_dentry);
}
