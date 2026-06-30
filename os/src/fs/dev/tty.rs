// os/src/fs/dev/tty.rs

use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::{DEVFS_DEV, TTY_INO, TTY_RDEV};
use crate::sbi::console_getchar;
use crate::syscall::{Errno, SysResult};
use crate::task::yield_current_task;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

const LF: usize = 0x0a;
const CR: usize = 0x0d;

pub(super) struct TtyInode;

impl InodeOp for TtyInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::CharDevice
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::CharDevice)
            .with_dev(DEVFS_DEV)
            .with_ino(TTY_INO)
            .with_mode(0o666)
            .with_rdev(TTY_RDEV))
    }

    fn read_at(&self, _path: &str, _off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let mut count = 0;
        while count < buf.len() {
            let c = console_getchar();
            match c {
                0 | 256.. => {
                    yield_current_task();
                    continue;
                }
                CR | LF => {
                    buf[count] = LF as u8;
                    count += 1;
                    break;
                }
                _ => {
                    buf[count] = c as u8;
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn write_at(&self, _path: &str, _off: usize, buf: &[u8]) -> SysResult<usize> {
        unsafe {
            print!("{}", core::str::from_utf8_unchecked(buf));
        }
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
