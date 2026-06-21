// os/src/fs/proc/stat.rs

use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::dirs::{proc_dev, proc_self_stat_ino, proc_stat_ino};
use crate::syscall::{Errno, SysResult};
use crate::task::{TASK_MANAGER, TaskStatus, current_task};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

pub(super) struct ProcStatInode;

impl InodeOp for ProcStatInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_proc_stat().len();
        Ok(KStat::minimal(size, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_stat_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_proc_stat();
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

pub(super) struct TaskStatInode {
    pid: Option<usize>,
}

impl TaskStatInode {
    pub(super) fn current() -> Self {
        Self { pid: None }
    }
}

impl InodeOp for TaskStatInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let content = generate_task_stat(self.pid)?;
        Ok(KStat::minimal(content.len(), InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_self_stat_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_task_stat(self.pid)?;
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

fn generate_proc_stat() -> String {
    let tasks = TASK_MANAGER.len();
    alloc::format!(
        "cpu  0 0 0 0 0 0 0 0 0 0\nintr 0\nctxt 0\nprocesses {}\n",
        tasks
    )
}

fn generate_task_stat(pid: Option<usize>) -> SysResult<String> {
    let task = match current_task() {
        Some(t) => t,
        None => return Err(Errno::ESRCH),
    };
    let task = if let Some(pid) = pid {
        TASK_MANAGER.get(pid).ok_or(Errno::ENOENT)?
    } else {
        task
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
        TaskStatus::Stopped => 'T',
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

    let ticks = task.elapsed_ticks();
    Ok(alloc::format!(
        "{} ({}) {} {} 0 0 0 0 0 0 0 0 0 {} {}{}\n",
        pid, comm, state, ppid, ticks, ticks, " 0".repeat(39)
    ))
}
