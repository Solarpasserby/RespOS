// os/src/fs/proc/meminfo.rs

use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::super::KStat;
use super::dirs::{proc_dev, proc_meminfo_ino};
use crate::syscall::{Errno, SysResult};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

pub(super) struct MeminfoInode;

impl InodeOp for MeminfoInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_meminfo().len();
        Ok(KStat::minimal(size, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_meminfo_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_meminfo();
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

fn generate_meminfo() -> String {
    let free_frames = crate::mm::free_frame_count();
    let page_size = crate::config::PAGE_SIZE;
    let cached_pages = crate::fs::page_cache_page_count();
    let dirty_pages = crate::fs::page_cache_dirty_page_count();
    let mem_total = crate::config::MEMORY_END - crate::config::MEMORY_START;
    let mem_free = free_frames * page_size;
    let mem_cached = cached_pages * page_size;
    let mem_dirty = dirty_pages * page_size;
    let heap_used = crate::mm::heap_allocated();

    let mut result = String::new();
    let _ = writeln!(result, "MemTotal:       {:8} kB", mem_total / 1024);
    let _ = writeln!(result, "MemFree:        {:8} kB", mem_free / 1024);
    let _ = writeln!(result, "MemAvailable:   {:8} kB", mem_free / 1024);
    let _ = writeln!(result, "Cached:         {:8} kB", mem_cached / 1024);
    let _ = writeln!(result, "Dirty:          {:8} kB", mem_dirty / 1024);
    let _ = writeln!(result, "KernelHeap:     {:8} kB", heap_used / 1024);
    result
}
