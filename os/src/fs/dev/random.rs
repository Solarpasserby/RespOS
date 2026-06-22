// os/src/fs/dev/random.rs

use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::{DEVFS_DEV, RANDOM_INO, RANDOM_RDEV, URANDOM_INO, URANDOM_RDEV};
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

pub(super) struct RandomInode {
    urandom: bool,
}

impl RandomInode {
    pub const fn random() -> Self {
        Self { urandom: false }
    }

    pub const fn urandom() -> Self {
        Self { urandom: true }
    }
}

impl InodeOp for RandomInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::CharDevice
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let (ino, rdev) = if self.urandom {
            (URANDOM_INO, URANDOM_RDEV)
        } else {
            (RANDOM_INO, RANDOM_RDEV)
        };
        Ok(KStat::minimal(0, InodeType::CharDevice)
            .with_dev(DEVFS_DEV)
            .with_ino(ino)
            .with_mode(0o666)
            .with_rdev(rdev))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let mut seed = crate::timer::get_time_ms()
            ^ off
            ^ if self.urandom {
                0x7572_616e_646f_6d
            } else {
                0x7261_6e64_6f6d
            };
        for (idx, byte) in buf.iter_mut().enumerate() {
            seed ^= seed << 7;
            seed ^= seed >> 9;
            seed ^= (idx + 1usize).wrapping_mul(0x9e37_79b9);
            *byte = seed as u8;
        }
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

    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}
