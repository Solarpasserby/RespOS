// os/src/fs/dev/loop_device.rs

use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::{DEVFS_DEV, LOOP_CONTROL_INO, LOOP_CONTROL_RDEV, LOOP0_INO, LOOP0_RDEV};
use crate::fs::FileOp;
use crate::mm::copy_to_user;
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

const LOOP_SET_FD: usize = 0x4c00;
const LOOP_CLR_FD: usize = 0x4c01;
const LOOP_SET_STATUS: usize = 0x4c02;
const LOOP_GET_STATUS: usize = 0x4c03;
const LOOP_CTL_GET_FREE: usize = 0x4c82;
const BLKGETSIZE64: usize = 0x8008_1272;
const BLKGETSIZE: usize = 0x1260;

static LOOP0_BACKEND: Mutex<Option<Arc<dyn FileOp>>> = Mutex::new(None);

pub struct LoopControlInode;

impl LoopControlInode {
    pub fn ioctl(&self, request: usize, _arg: usize) -> SysResult<usize> {
        match request {
            LOOP_CTL_GET_FREE => {
                if LOOP0_BACKEND.lock().is_none() {
                    Ok(0)
                } else {
                    Err(Errno::ENODEV)
                }
            }
            _ => Err(Errno::EINVAL),
        }
    }
}

impl InodeOp for LoopControlInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::CharDevice
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::CharDevice)
            .with_dev(DEVFS_DEV)
            .with_ino(LOOP_CONTROL_INO)
            .with_mode(0o666)
            .with_rdev(LOOP_CONTROL_RDEV))
    }

    fn read_at(&self, _path: &str, _off: usize, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::ENOSYS)
    }

    fn write_at(&self, _path: &str, _off: usize, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::ENOSYS)
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

pub struct LoopInode {
    id: usize,
}

impl LoopInode {
    pub fn new(id: usize) -> Self {
        Self { id }
    }

    pub fn ioctl(&self, request: usize, arg: usize) -> SysResult<usize> {
        match request {
            LOOP_SET_FD => {
                let task = current_task().expect("[kernel] current task is None.");
                let file = task.get_fd_entry(arg)?.file;
                *LOOP0_BACKEND.lock() = Some(file);
                Ok(0)
            }
            LOOP_CLR_FD => {
                if LOOP0_BACKEND.lock().take().is_some() {
                    Ok(0)
                } else {
                    Err(Errno::ENXIO)
                }
            }
            LOOP_SET_STATUS => Ok(0),
            LOOP_GET_STATUS => {
                if LOOP0_BACKEND.lock().is_some() {
                    Ok(0)
                } else {
                    Err(Errno::ENXIO)
                }
            }
            request if request & 0xffff == BLKGETSIZE64 & 0xffff => {
                let size = loop_backend()?.get_stat()?.size as u64;
                copy_to_user(arg as *mut u64, &size as *const u64, 1)?;
                Ok(0)
            }
            request if request & 0xffff == BLKGETSIZE => {
                let sectors = (loop_backend()?.get_stat()?.size / 512) as usize;
                copy_to_user(arg as *mut usize, &sectors as *const usize, 1)?;
                Ok(0)
            }
            _ => Err(Errno::EINVAL),
        }
    }
}

impl InodeOp for LoopInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::BlockDevice
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::BlockDevice)
            .with_dev(DEVFS_DEV)
            .with_ino(LOOP0_INO + self.id as u64)
            .with_mode(0o666)
            .with_rdev(LOOP0_RDEV + self.id as u64))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let file = loop_backend()?;
        file.seek(off as isize)?;
        file.read(buf)
    }

    fn write_at(&self, _path: &str, off: usize, buf: &[u8]) -> SysResult<usize> {
        let file = loop_backend()?;
        file.seek(off as isize)?;
        file.write(buf)
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

fn loop_backend() -> SysResult<Arc<dyn FileOp>> {
    LOOP0_BACKEND.lock().as_ref().cloned().ok_or(Errno::ENODEV)
}
