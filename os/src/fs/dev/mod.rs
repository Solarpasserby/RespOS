// os/src/fs/dev/mod.rs

//! 虚拟 devfs 设备文件系统。

mod null;

use super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::KStat;
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use null::NullInode;

// ── /dev ────────────────────────────────────────────────────────────

struct DevDirInode;

impl InodeOp for DevDirInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat {
            size: 0,
            ty: InodeType::Directory,
        })
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        if name == "null" {
            Ok(Arc::new(NullInode))
        } else {
            Err(Errno::ENOENT)
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            LinuxDirent64 {
                d_ino: 0,
                d_off: 1,
                d_reclen: 24,
                d_type: InodeType::Directory as u8,
                d_name: b".\0".to_vec(),
            },
            LinuxDirent64 {
                d_ino: 0,
                d_off: 2,
                d_reclen: 24,
                d_type: InodeType::Directory as u8,
                d_name: b"..\0".to_vec(),
            },
            LinuxDirent64 {
                d_ino: 0,
                d_off: 3,
                d_reclen: 24,
                d_type: InodeType::CharDevice as u8,
                d_name: b"null\0".to_vec(),
            },
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

/// 在根文件系统中创建 /dev 目录及 /dev/null 设备节点。
pub fn init_devfs(root: Arc<Dentry>) {
    let dev_dentry = Arc::new(Dentry::new(
        "/dev".into(),
        Some(root.clone()),
        Arc::new(DevDirInode),
    ));
    root.insert_child("dev", dev_dentry.clone());

    let null_dentry = Arc::new(Dentry::new(
        "/dev/null".into(),
        Some(dev_dentry.clone()),
        Arc::new(NullInode),
    ));
    dev_dentry.insert_child("null", null_dentry);
}
