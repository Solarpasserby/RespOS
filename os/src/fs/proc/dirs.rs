// os/src/fs/proc/dirs.rs

use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::super::KStat;
use super::exe::ProcExeInode;
use super::smaps::SmapsInode;
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;

const PROC_ROOT_INO: u64 = 1;
const PROC_SELF_INO: u64 = 2;
const PROC_SELF_SMAPS_INO: u64 = 3;
const PROC_SELF_EXE_INO: u64 = 4;
const PROC_DEV: u64 = 0x100;

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
        } else {
            Err(Errno::ENOENT)
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(PROC_ROOT_INO, 1, b".\0"),
            dir_entry(2, 2, b"..\0"),
            entry(PROC_SELF_INO, InodeType::Directory, 3, b"self\0"),
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
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(PROC_SELF_INO, 1, b".\0"),
            dir_entry(PROC_ROOT_INO, 2, b"..\0"),
            entry(PROC_SELF_SMAPS_INO, InodeType::Regular, 3, b"smaps\0"),
            entry(PROC_SELF_EXE_INO, InodeType::SymLink, 4, b"exe\0"),
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

// ── helpers ───────────────────────────────────────────────────────

pub(super) fn proc_self_smaps_ino() -> u64 {
    PROC_SELF_SMAPS_INO
}

pub(super) fn proc_self_exe_ino() -> u64 {
    PROC_SELF_EXE_INO
}

pub(super) fn proc_dev() -> u64 {
    PROC_DEV
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
