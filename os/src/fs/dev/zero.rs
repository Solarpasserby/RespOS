// os/src/fs/dev/zero.rs

use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::{DEVFS_DEV, ZERO_INO, ZERO_RDEV};
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

pub(super) struct ZeroInode;

impl InodeOp for ZeroInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::CharDevice
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::CharDevice)
            .with_dev(DEVFS_DEV)
            .with_ino(ZERO_INO)
            .with_mode(0o666)
            .with_rdev(ZERO_RDEV))
    }

    fn read_at(&self, _path: &str, _off: usize, buf: &mut [u8]) -> SysResult<usize> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write_at(&self, _path: &str, _off: usize, buf: &[u8]) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn truncate(&self, _path: &str, _size: usize) -> SysResult<usize> {
        Ok(0)
    }

    fn lookup(&self, _parent_path: &str, _name: &str) -> SysResult<Arc<dyn InodeOp>> {
        Err(Errno::ENOTDIR)
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Err(Errno::ENOTDIR)
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
