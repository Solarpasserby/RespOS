// os/src/fs/proc/exe.rs

use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::super::KStat;
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

pub(super) struct ProcExeInode;

impl InodeOp for ProcExeInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::SymLink
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let task = current_task().expect("[procfs] no current task");
        let exe = task.exe_path();
        Ok(KStat {
            size: exe.len(),
            ty: InodeType::SymLink,
        })
    }

    fn read_link(&self, _path: &str) -> SysResult<String> {
        let task = current_task().expect("[procfs] no current task");
        Ok(task.exe_path())
    }

    fn read_at(&self, _path: &str, _off: usize, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::EACCES)
    }
    fn write_at(&self, _path: &str, _off: usize, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EACCES)
    }
    fn truncate(&self, _path: &str, _size: usize) -> SysResult<usize> {
        Err(Errno::EACCES)
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
