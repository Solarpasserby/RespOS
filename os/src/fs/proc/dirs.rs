// os/src/fs/proc/dirs.rs

use super::super::{File, KStat};
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::cpuinfo::CpuinfoInode;
use super::exe::ProcExeInode;
use super::meminfo::MeminfoInode;
use super::mounts::MountsInode;
use super::smaps::SmapsInode;
use super::stat::{ProcStatInode, TaskStatInode};
use crate::syscall::{Errno, SysResult};
use crate::task::{TASK_MANAGER, TaskStatus, current_task};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

const PROC_ROOT_INO: u64 = 1;
const PROC_SELF_INO: u64 = 2;
const PROC_SELF_SMAPS_INO: u64 = 3;
const PROC_SELF_EXE_INO: u64 = 4;
const PROC_MEMINFO_INO: u64 = 5;
const PROC_MOUNTS_INO: u64 = 6;
const PROC_STAT_INO: u64 = 7;
const PROC_SELF_STAT_INO: u64 = 8;
const PROC_SYS_INO: u64 = 9;
const PROC_SYS_KERNEL_INO: u64 = 10;
const PROC_SYS_KERNEL_PID_MAX_INO: u64 = 11;
const PROC_CPUINFO_INO: u64 = 12;
const PROC_SELF_FD_INO: u64 = 12;
const PROC_SYS_FS_INO: u64 = 13;
const PROC_SYS_FS_PIPE_USER_PAGES_SOFT_INO: u64 = 14;
const PROC_PID_DIR_INO_BASE: u64 = 0x10000;
const PROC_PID_STAT_INO_BASE: u64 = 0x20000;
const PROC_DEV: u64 = 0x100;
const PID_MAX_CONTENT: &str = "4194304\n";
const PIPE_USER_PAGES_SOFT_CONTENT: &str = "128\n";

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
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(PROC_DEV)
            .with_ino(PROC_ROOT_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        if name == "self" {
            Ok(Arc::new(ProcSelfInode))
        } else if name == "meminfo" {
            Ok(Arc::new(MeminfoInode))
        } else if name == "mounts" {
            Ok(Arc::new(MountsInode))
        } else if name == "stat" {
            Ok(Arc::new(ProcStatInode))
        } else if name == "cpuinfo" {
            Ok(Arc::new(CpuinfoInode))
        } else if name == "sys" {
            Ok(Arc::new(ProcSysInode))
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
            dir_entry(PROC_ROOT_INO, 1, b".\0"),
            dir_entry(2, 2, b"..\0"),
            entry(PROC_SELF_INO, InodeType::Directory, 3, b"self\0"),
            entry(PROC_MEMINFO_INO, InodeType::Regular, 4, b"meminfo\0"),
            entry(PROC_MOUNTS_INO, InodeType::Regular, 5, b"mounts\0"),
            entry(PROC_STAT_INO, InodeType::Regular, 6, b"stat\0"),
            entry(PROC_CPUINFO_INO, InodeType::Regular, 7, b"cpuinfo\0"),
            entry(PROC_SYS_INO, InodeType::Directory, 8, b"sys\0"),
        ];
        let pids = core::cell::RefCell::new(Vec::new());
        TASK_MANAGER.for_each(|task| {
            // 只保留进程 leader（tgid == tid），避免线程重复出现
            if task.tid() == task.tgid() {
                pids.borrow_mut().push(task.tid());
            }
        });
        let mut off: i64 = 9;
        for pid in pids.into_inner() {
            let name = alloc::format!("{}\0", pid).into_bytes();
            entries.push(entry(
                proc_pid_dir_ino(pid),
                InodeType::Directory,
                off,
                &name,
            ));
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

// ── /proc/sys/kernel ──────────────────────────────────────────────

pub(super) struct ProcSysInode;

impl InodeOp for ProcSysInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(PROC_DEV)
            .with_ino(PROC_SYS_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        match name {
            "kernel" => Ok(Arc::new(ProcSysKernelInode)),
            "fs" => Ok(Arc::new(ProcSysFsInode)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(PROC_SYS_INO, 1, b".\0"),
            dir_entry(PROC_ROOT_INO, 2, b"..\0"),
            entry(PROC_SYS_KERNEL_INO, InodeType::Directory, 3, b"kernel\0"),
            entry(PROC_SYS_FS_INO, InodeType::Directory, 4, b"fs\0"),
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

pub(super) struct ProcSysFsInode;

impl InodeOp for ProcSysFsInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(PROC_DEV)
            .with_ino(PROC_SYS_FS_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        match name {
            "pipe-user-pages-soft" => Ok(Arc::new(ProcPipeUserPagesSoftInode)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(PROC_SYS_FS_INO, 1, b".\0"),
            dir_entry(PROC_SYS_INO, 2, b"..\0"),
            entry(
                PROC_SYS_FS_PIPE_USER_PAGES_SOFT_INO,
                InodeType::Regular,
                3,
                b"pipe-user-pages-soft\0",
            ),
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

pub(super) struct ProcSysKernelInode;

impl InodeOp for ProcSysKernelInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(PROC_DEV)
            .with_ino(PROC_SYS_KERNEL_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        match name {
            "pid_max" => Ok(Arc::new(ProcPidMaxInode)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(PROC_SYS_KERNEL_INO, 1, b".\0"),
            dir_entry(PROC_SYS_INO, 2, b"..\0"),
            entry(
                PROC_SYS_KERNEL_PID_MAX_INO,
                InodeType::Regular,
                3,
                b"pid_max\0",
            ),
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

pub(super) struct ProcPidMaxInode;

impl InodeOp for ProcPidMaxInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(PID_MAX_CONTENT.len(), InodeType::Regular)
            .with_dev(PROC_DEV)
            .with_ino(PROC_SYS_KERNEL_PID_MAX_INO)
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let bytes = PID_MAX_CONTENT.as_bytes();
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

pub(super) struct ProcPipeUserPagesSoftInode;

impl InodeOp for ProcPipeUserPagesSoftInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(
            KStat::minimal(PIPE_USER_PAGES_SOFT_CONTENT.len(), InodeType::Regular)
                .with_dev(PROC_DEV)
                .with_ino(PROC_SYS_FS_PIPE_USER_PAGES_SOFT_INO)
                .with_mode(0o444),
        )
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let bytes = PIPE_USER_PAGES_SOFT_CONTENT.as_bytes();
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
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(PROC_DEV)
            .with_ino(PROC_SELF_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        match name {
            "smaps" => Ok(Arc::new(SmapsInode)),
            "exe" => Ok(Arc::new(ProcExeInode)),
            "mounts" => Ok(Arc::new(MountsInode)),
            "stat" => Ok(Arc::new(TaskStatInode::current())),
            "fd" => Ok(Arc::new(ProcSelfFdInode)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(PROC_SELF_INO, 1, b".\0"),
            dir_entry(PROC_ROOT_INO, 2, b"..\0"),
            entry(PROC_SELF_SMAPS_INO, InodeType::Regular, 3, b"smaps\0"),
            entry(PROC_SELF_EXE_INO, InodeType::SymLink, 4, b"exe\0"),
            entry(PROC_MOUNTS_INO, InodeType::Regular, 5, b"mounts\0"),
            entry(PROC_SELF_STAT_INO, InodeType::Regular, 6, b"stat\0"),
            entry(PROC_SELF_FD_INO, InodeType::Directory, 7, b"fd\0"),
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

pub(super) struct ProcSelfFdInode;

impl InodeOp for ProcSelfFdInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(PROC_DEV)
            .with_ino(PROC_SELF_FD_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        let fd = name.parse::<usize>().map_err(|_| Errno::ENOENT)?;
        let task = current_task().ok_or(Errno::ENOENT)?;
        task.get_fd_entry(fd)?;
        Ok(Arc::new(ProcSelfFdEntryInode { fd }))
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        let task = current_task().ok_or(Errno::ENOENT)?;
        let mut entries = vec![
            dir_entry(PROC_SELF_FD_INO, 1, b".\0"),
            dir_entry(PROC_SELF_INO, 2, b"..\0"),
        ];
        let mut off = 3i64;
        for fd in task.open_fds() {
            let name = alloc::format!("{}\0", fd).into_bytes();
            entries.push(entry(
                PROC_SELF_FD_INO + 100 + fd as u64,
                InodeType::SymLink,
                off,
                &name,
            ));
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::EACCES)
    }
}

pub(super) struct ProcSelfFdEntryInode {
    fd: usize,
}

impl InodeOp for ProcSelfFdEntryInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::SymLink
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(64, InodeType::SymLink)
            .with_dev(PROC_DEV)
            .with_ino(PROC_SELF_FD_INO + 100 + self.fd as u64)
            .with_mode(0o777))
    }

    fn read_link(&self, _path: &str) -> SysResult<String> {
        let task = current_task().ok_or(Errno::ENOENT)?;
        let file = task.get_fd_entry(self.fd)?.get_file();
        if let Some(file) = file.as_any().downcast_ref::<File>() {
            return Ok(file.path().global_abs_path());
        }
        Ok(alloc::format!("anon_inode:[fd:{}]", self.fd))
    }

    fn read_at(&self, _path: &str, _off: usize, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
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
        if TASK_MANAGER.get(self.pid).is_none() {
            return Err(Errno::ENOENT);
        }
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(PROC_DEV)
            .with_ino(proc_pid_dir_ino(self.pid))
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        if TASK_MANAGER.get(self.pid).is_none() {
            return Err(Errno::ENOENT);
        }
        if name == "stat" {
            Ok(Arc::new(ProcPidStatInode { pid: self.pid }))
        } else {
            Err(Errno::ENOENT)
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        if TASK_MANAGER.get(self.pid).is_none() {
            return Err(Errno::ENOENT);
        }
        Ok(vec![
            dir_entry(proc_pid_dir_ino(self.pid), 1, b".\0"),
            dir_entry(PROC_ROOT_INO, 2, b"..\0"),
            entry(
                proc_pid_stat_ino(self.pid),
                InodeType::Regular,
                3,
                b"stat\0",
            ),
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
    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
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
        let content = generate_pid_stat(self.pid)?;
        Ok(KStat::minimal(content.len(), InodeType::Regular)
            .with_dev(PROC_DEV)
            .with_ino(proc_pid_stat_ino(self.pid))
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_pid_stat(self.pid)?;
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

fn generate_pid_stat(pid: usize) -> SysResult<String> {
    let Some(task) = TASK_MANAGER.get(pid) else {
        return Err(Errno::ENOENT);
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
    let _ = write!(result, "{} ({}) {} {}", pid, comm, state, ppid);
    for _ in 0..48 {
        result.push_str(" 0");
    }
    result.push('\n');
    Ok(result)
}

// ── helpers ───────────────────────────────────────────────────────

pub(super) fn proc_self_smaps_ino() -> u64 {
    PROC_SELF_SMAPS_INO
}

pub(super) fn proc_self_exe_ino() -> u64 {
    PROC_SELF_EXE_INO
}

pub(super) fn proc_meminfo_ino() -> u64 {
    PROC_MEMINFO_INO
}

pub(super) fn proc_mounts_ino() -> u64 {
    PROC_MOUNTS_INO
}

pub(super) fn proc_stat_ino() -> u64 {
    PROC_STAT_INO
}

pub(super) fn proc_cpuinfo_ino() -> u64 {
    PROC_CPUINFO_INO
}

pub(super) fn proc_self_stat_ino() -> u64 {
    PROC_SELF_STAT_INO
}

pub(super) fn proc_dev() -> u64 {
    PROC_DEV
}

fn proc_pid_dir_ino(pid: usize) -> u64 {
    PROC_PID_DIR_INO_BASE + pid as u64
}

fn proc_pid_stat_ino(pid: usize) -> u64 {
    PROC_PID_STAT_INO_BASE + pid as u64
}

fn entry(ino: u64, ty: InodeType, off: i64, name: &[u8]) -> LinuxDirent64 {
    let reclen = (19 + name.len() + 7) & !7;
    LinuxDirent64 {
        d_ino: ino,
        d_off: off,
        d_reclen: reclen as u16,
        d_type: ty as u8,
        d_name: name.to_vec(),
    }
}

fn dir_entry(ino: u64, off: i64, name: &[u8]) -> LinuxDirent64 {
    entry(ino, InodeType::Directory, off, name)
}
