// os/src/fs/proc/stat.rs

use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::super::KStat;
use crate::syscall::{Errno, SysResult};
use crate::task::{TaskStatus, current_task};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

pub(super) struct StatInode;

impl InodeOp for StatInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_stat().len();
        Ok(KStat {
            size,
            ty: InodeType::Regular,
        })
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_stat();
        let bytes = content.as_bytes();
        if off >= bytes.len() {
            return Ok(0);
        }
        let n = buf.len().min(bytes.len() - off);
        buf[..n].copy_from_slice(&bytes[off..off + n]);
        Ok(n)
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

fn generate_stat() -> String {
    let task = match current_task() {
        Some(t) => t,
        None => return String::new(),
    };

    let pid = task.tid();
    let ppid = task.op_parent(|p| {
        p.as_ref()
            .and_then(|w| w.upgrade())
            .map(|t| t.tid())
            .unwrap_or(0)
    });
    let state = match task.status() {
        TaskStatus::Ready | TaskStatus::Running => 'R',
        TaskStatus::Blocked => 'S',
        TaskStatus::Exited => 'Z',
    };

    let comm = {
        let path = task.exe_path();
        path.rsplit('/')
            .next()
            .unwrap_or(&path)
            .chars()
            .take(15)
            .collect::<String>()
    };

    let zeros = " 0".repeat(48);

    alloc::format!(
        "{} ({}) {} {}{}\n",
        pid, comm, state, ppid, zeros
    )
}
