//! 极简内存根目录（Stage 4 无盘降级用）。
//!
//! 2K1000LA 真机 Stage 4 还没接 AHCI 块设备，`init_root_fs` 无 ext4 可用时用这个空目录
//! 根作为降级：目录操作返回空/ENOENT/EACCES，让用户态仍能启动，磁盘相关 syscall 优雅失败。

use crate::fs::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64, SuperBlockOp};
use crate::fs::KStat;
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;

/// 空根目录：只有 `.` 和 `..`，不支持创建/查找子项。
pub struct EmptyRootInode;

fn dir_entry(ino: u64, off: i64, name: &[u8]) -> LinuxDirent64 {
    let reclen = (19 + name.len() + 7) & !7;
    LinuxDirent64 {
        d_ino: ino,
        d_off: off,
        d_reclen: reclen as u16,
        d_type: InodeType::Directory as u8,
        d_name: name.to_vec(),
    }
}

impl InodeOp for EmptyRootInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_nlink(2)
            .with_mode(0o555))
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

    fn lookup(&self, _parent_path: &str, _name: &str) -> SysResult<Arc<dyn InodeOp>> {
        Err(Errno::ENOENT)
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![dir_entry(1, 1, b".\0"), dir_entry(1, 2, b"..\0")])
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

    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

/// 空根目录的超级块。
pub struct EmptyRootSuperBlock;

impl SuperBlockOp for EmptyRootSuperBlock {
    fn root_inode(&self) -> Arc<dyn InodeOp> {
        Arc::new(EmptyRootInode)
    }

    fn sync(&self) -> SysResult {
        Ok(())
    }
}
