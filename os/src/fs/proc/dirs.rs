// os/src/fs/proc/dirs.rs

use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::super::KStat;
use super::exe::ProcExeInode;
use super::meminfo::MeminfoInode;
use super::mounts::MountsInode;
use super::smaps::SmapsInode;
use super::stat::StatInode;
use crate::syscall::{Errno, SysResult};
use crate::task::{TASK_MANAGER, TaskStatus};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

// ── /proc ─────────────────────────────────────────────────────────

pub(super) struct ProcDirInode;

impl InodeOp for ProcDirInode {
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
        if name == "self" {
            Ok(Arc::new(ProcSelfInode))
        } else if name == "meminfo" {
            Ok(Arc::new(MeminfoInode))
        } else if name == "mounts" {
            Ok(Arc::new(MountsInode))
        } else if let Ok(pid) = name.parse::<usize>() {
            if TASK_MANAGER.get(pid).is_some() {
                Ok(Arc::new(ProcPidDirInode { pid }))
            } else {
                Err(Errno::ENOENT)
            }
        } else {
            Err(Errno::ENOENT)
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        let mut entries = vec![
            dir_entry(1, b".\0"),
            dir_entry(2, b"..\0"),
            entry(InodeType::Directory, 3, b"self\0"),
            entry(InodeType::Regular, 4, b"meminfo\0"),
            entry(InodeType::Regular, 5, b"mounts\0"),
        ];
        let pids = core::cell::RefCell::new(Vec::new());
        TASK_MANAGER.for_each(|task| {
            // 只保留进程 leader（tgid == tid），避免线程重复出现
            if task.tid() == task.tgid() {
                pids.borrow_mut().push(task.tid());
            }
        });
        let mut off: i64 = 6;
        for pid in pids.into_inner() {
            let name = alloc::format!("{}\0", pid).into_bytes();
            entries.push(entry(InodeType::Directory, off, &name));
            off += 1;
        }
        Ok(entries)
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

// ── /proc/self ────────────────────────────────────────────────────

pub(super) struct ProcSelfInode;

impl InodeOp for ProcSelfInode {
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
        match name {
            "smaps" => Ok(Arc::new(SmapsInode)),
            "exe" => Ok(Arc::new(ProcExeInode)),
            "mounts" => Ok(Arc::new(MountsInode)),
            "stat" => Ok(Arc::new(StatInode)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(1, b".\0"),
            dir_entry(2, b"..\0"),
            entry(InodeType::Regular, 3, b"smaps\0"),
            entry(InodeType::SymLink, 4, b"exe\0"),
            entry(InodeType::Regular, 5, b"mounts\0"),
            entry(InodeType::Regular, 6, b"stat\0"),
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

// ── /proc/<pid> ────────────────────────────────────────────────────

pub(super) struct ProcPidDirInode {
    pub pid: usize,
}

impl InodeOp for ProcPidDirInode {
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
        if name == "stat" {
            Ok(Arc::new(ProcPidStatInode { pid: self.pid }))
        } else {
            Err(Errno::ENOENT)
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(1, b".\0"),
            dir_entry(2, b"..\0"),
            entry(InodeType::Regular, 3, b"stat\0"),
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

pub(super) struct ProcPidStatInode {
    pid: usize,
}

impl InodeOp for ProcPidStatInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_pid_stat(self.pid).len();
        Ok(KStat {
            size,
            ty: InodeType::Regular,
        })
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_pid_stat(self.pid);
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

fn generate_pid_stat(pid: usize) -> String {
    let Some(task) = TASK_MANAGER.get(pid) else {
        return String::new();
    };

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

    let mut result = String::new();
    let _ = write!(
        result,
        "{} ({}) {} {}",
        pid, comm, state, ppid
    );
    for _ in 0..48 {
        result.push_str(" 0");
    }
    result.push('\n');
    result
}

// ── helpers ───────────────────────────────────────────────────────

fn entry(ty: InodeType, off: i64, name: &[u8]) -> LinuxDirent64 {
    let reclen = (19 + name.len() + 7) & !7;
    LinuxDirent64 {
        d_ino: 0,
        d_off: off,
        d_reclen: reclen as u16,
        d_type: ty as u8,
        d_name: name.to_vec(),
    }
}

fn dir_entry(off: i64, name: &[u8]) -> LinuxDirent64 {
    entry(InodeType::Directory, off, name)
}
