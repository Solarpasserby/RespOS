// os/src/fs/proc/maps.rs

use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::dirs::{proc_dev, proc_self_maps_ino, proc_self_pagemap_ino, proc_self_status_ino};
use crate::config::PAGE_SIZE;
use crate::mm::MapPermission;
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

pub(super) struct MapsInode;
pub(super) struct StatusInode;
pub(super) struct PagemapInode;

impl InodeOp for MapsInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_maps().len();
        Ok(KStat::minimal(size, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_self_maps_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        read_string_at(generate_maps(), off, buf)
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

impl InodeOp for StatusInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_status().len();
        Ok(KStat::minimal(size, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_self_status_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        read_string_at(generate_status(), off, buf)
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

impl InodeOp for PagemapInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(usize::MAX / 2, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_self_pagemap_ino())
            .with_mode(0o400))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let mut written = 0;
        while written + 8 <= buf.len() {
            let entry_off = off + written;
            let vpn = entry_off / 8;
            let present = is_vpn_mapped(vpn);
            let entry = if present { 1u64 << 63 } else { 0 };
            buf[written..written + 8].copy_from_slice(&entry.to_ne_bytes());
            written += 8;
        }
        Ok(written)
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

fn read_string_at(content: String, off: usize, buf: &mut [u8]) -> SysResult<usize> {
    let bytes = content.as_bytes();
    if off >= bytes.len() {
        return Ok(0);
    }
    let n = buf.len().min(bytes.len() - off);
    buf[..n].copy_from_slice(&bytes[off..off + n]);
    Ok(n)
}

fn generate_maps() -> String {
    let task = match current_task() {
        Some(task) => task,
        None => return String::new(),
    };

    let mut result = String::new();
    task.op_memory_set_read(|mm| {
        mm.each_area(|start, end, perm, shared, _locked| {
            let perms = perm_to_maps_str(perm, shared);
            let _ = writeln!(result, "{:x}-{:x} {} 00000000 00:00 0", start, end, perms);
        });
    });
    result
}

fn generate_status() -> String {
    let task = match current_task() {
        Some(task) => task,
        None => return String::new(),
    };

    let mut locked_bytes = 0usize;
    task.op_memory_set_read(|mm| {
        mm.each_area(|start, end, _perm, _shared, locked| {
            if locked {
                locked_bytes += end.saturating_sub(start);
            }
        });
    });

    let mut result = String::new();
    let _ = writeln!(
        result,
        "Name:\t{}",
        task.exe_path().rsplit('/').next().unwrap_or("")
    );
    let _ = writeln!(result, "Pid:\t{}", task.tid());
    let _ = writeln!(result, "VmLck:\t{:8} kB", locked_bytes / 1024);
    result
}

fn is_vpn_mapped(vpn: usize) -> bool {
    let Some(task) = current_task() else {
        return false;
    };
    let addr = vpn.saturating_mul(PAGE_SIZE);
    let mut mapped = false;
    task.op_memory_set_read(|mm| {
        mm.each_area(|start, end, _perm, _shared, _locked| {
            if addr >= start && addr < end {
                mapped = true;
            }
        });
    });
    mapped
}

fn perm_to_maps_str(perm: MapPermission, shared: bool) -> String {
    let mut s = String::new();
    s.push(if perm.contains(MapPermission::READ) {
        'r'
    } else {
        '-'
    });
    s.push(if perm.contains(MapPermission::WRITE) {
        'w'
    } else {
        '-'
    });
    s.push(if perm.contains(MapPermission::EXECUTE) {
        'x'
    } else {
        '-'
    });
    s.push(if shared { 's' } else { 'p' });
    s
}
