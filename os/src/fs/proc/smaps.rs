// os/src/fs/proc/smaps.rs

use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::dirs::{proc_dev, proc_self_smaps_ino};
use crate::config::PAGE_SIZE;
use crate::mm::MapPermission;
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

pub(super) struct SmapsInode;

impl InodeOp for SmapsInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_smaps().len();
        Ok(KStat::minimal(size, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_self_smaps_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_smaps();
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

fn generate_smaps() -> String {
    let task = match current_task() {
        Some(t) => t,
        None => return String::new(),
    };

    let mut result = String::new();
    task.op_memory_set_read(|mm| {
        mm.each_area(|start, end, perm, _shared, _locked| {
            let p = perm_to_smaps_str(perm);
            let size_kb = end.saturating_sub(start).div_ceil(PAGE_SIZE) * (PAGE_SIZE / 1024);
            let private_dirty_kb = if perm.contains(MapPermission::WRITE) {
                size_kb
            } else {
                0
            };
            let pathname = if perm.contains(MapPermission::WRITE) {
                " [heap]"
            } else {
                ""
            };
            let _ = writeln!(
                result,
                "{:016x}-{:016x} {} 00000000 00:00 0{}",
                start, end, p, pathname
            );
            let _ = writeln!(result, "Size: {:>17} kB", size_kb);
            let _ = writeln!(result, "Rss: {:>18} kB", size_kb);
            let _ = writeln!(result, "Private_Dirty: {:>8} kB", private_dirty_kb);
        });
    });
    result
}

fn perm_to_smaps_str(perm: MapPermission) -> &'static str {
    let r = perm.contains(MapPermission::READ);
    let w = perm.contains(MapPermission::WRITE);
    let x = perm.contains(MapPermission::EXECUTE);
    match (r, w, x) {
        (true, true, true) => "rwxp",
        (true, true, false) => "rw-p",
        (true, false, true) => "r-xp",
        (true, false, false) => "r--p",
        (false, true, true) => "-wxp",
        (false, true, false) => "-w-p",
        (false, false, true) => "--xp",
        _ => "---p",
    }
}
