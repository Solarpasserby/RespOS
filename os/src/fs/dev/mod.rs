// os/src/fs/dev/mod.rs

//! 虚拟 devfs 设备文件系统。
//!
//! - `null`  — `/dev/null`，丢弃写入，读取始终返回 EOF

mod null;

const DEVFS_DEV: u64 = 0x400;
const DEVFS_SUPER_MAGIC: i64 = 0x1373;
const DEV_DIR_INO: u64 = 1;
const NULL_INO: u64 = 2;
const NULL_RDEV: u64 = (1 << 8) | 3;

use super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64, SuperBlockOp};
use super::{KStat, Statfs64};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use null::NullInode;

use crate::fs::mount::{self, Mount, VfsMount, get_mount_by_dentry};
use crate::syscall::{Errno, SysResult};

// ── /dev ─────────────────────────────────────────────────────────────

struct DevDirInode;

impl InodeOp for DevDirInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(DEVFS_DEV)
            .with_ino(DEV_DIR_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        match name {
            "null" => Ok(Arc::new(NullInode)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(DEV_DIR_INO, 1, b".\0"),
            dir_entry(2, 2, b"..\0"),
            entry(NULL_INO, InodeType::CharDevice, 3, b"null\0"),
        ])
    }

    fn read_at(&self, _path: &str, _off: usize, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }
    fn write_at(&self, _path: &str, _off: usize, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EACCES)
    }
    fn truncate(&self, _path: &str, _size: usize) -> SysResult<usize> {
        Err(Errno::EACCES)
    }
    fn create(
        &self,
        _parent_path: &str,
        _name: &str,
        _ty: InodeType,
    ) -> SysResult<Arc<dyn InodeOp>> {
        Err(Errno::EACCES)
    }
    fn link(&self, _old_path: &str, _bare_dentry: Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
    fn unlink(&self, _valid_dentry: Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

struct DevSuperBlock;

impl SuperBlockOp for DevSuperBlock {
    fn root_inode(&self) -> Arc<dyn InodeOp> {
        Arc::new(DevDirInode)
    }

    fn sync(&self) {}

    fn statfs(&self) -> SysResult<Statfs64> {
        Ok(Statfs64 {
            f_type: DEVFS_SUPER_MAGIC,
            f_bsize: crate::config::PAGE_SIZE as i64,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 2,
            f_ffree: 0,
            f_namelen: 255,
            f_frsize: crate::config::PAGE_SIZE as i64,
            ..Default::default()
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────

fn entry(ino: u64, ty: InodeType, off: i64, name: &[u8]) -> LinuxDirent64 {
    let reclen = (19 + name.len() + 7) & !7;
    LinuxDirent64 {
        d_ino: ino,
        d_off: off,
        d_reclen: reclen as u16,
        d_type: ty as u8,
        d_name: name.to_vec(),
    }
}

fn dir_entry(ino: u64, off: i64, name: &[u8]) -> LinuxDirent64 {
    entry(ino, InodeType::Directory, off, name)
}

// ── init ──────────────────────────────────────────────────────────────

/// 在根文件系统中创建 /dev/null 目录树。
pub fn init_devfs(root: Arc<Dentry>) {
    let dev_mountpoint = Arc::new(Dentry::new(
        "/dev".into(),
        Some(root.clone()),
        Arc::new(DevDirInode),
    ));
    root.insert_child("dev", dev_mountpoint.clone());
    mount::pin_vfs_dentry(dev_mountpoint.clone());

    let dev_root = Arc::new(Dentry::new("/".into(), None, Arc::new(DevDirInode)));
    mount::pin_vfs_dentry(dev_root.clone());
    let dev_mount = VfsMount::new(dev_root, Arc::new(DevSuperBlock), 0);
    let parent_mount = get_mount_by_dentry(&root).expect("[devfs] root mount is not initialized");
    mount::add_mount(Mount::new_child(dev_mountpoint, dev_mount, parent_mount));
}
