use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::super::KStat;
use super::dirs::{proc_dev, proc_respos_perf_ino};
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

pub(super) struct PerfStatsInode;

impl InodeOp for PerfStatsInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(
            KStat::minimal(crate::perf::render().len(), InodeType::Regular)
                .with_dev(proc_dev())
                .with_ino(proc_respos_perf_ino())
                .with_mode(0o644),
        )
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = crate::perf::render();
        let bytes = content.as_bytes();
        if off >= bytes.len() {
            return Ok(0);
        }
        let n = buf.len().min(bytes.len() - off);
        buf[..n].copy_from_slice(&bytes[off..off + n]);
        Ok(n)
    }

    fn write_at(&self, _path: &str, off: usize, buf: &[u8]) -> SysResult<usize> {
        if off != 0 {
            return Err(Errno::EINVAL);
        }
        let command = core::str::from_utf8(buf).map_err(|_| Errno::EINVAL)?;
        #[cfg(feature = "debug_traces")]
        if command.trim() == "fail_writeback" {
            crate::fs::page_cache::arm_writeback_fault();
            return Ok(buf.len());
        }
        #[cfg(all(feature = "heap_magazine", feature = "perf_counters"))]
        if command.trim() == "drain_heap_magazines" {
            let reclaimed = crate::mm::drain_heap_magazines();
            crate::perf::heap_magazine_reclaim_blocks(reclaimed);
            return Ok(buf.len());
        }
        #[cfg(feature = "io_buffer_pool")]
        if command.trim() == "drain_io_buffers" {
            let _ = crate::mm::drain_io_buffers();
            return Ok(buf.len());
        }
        if command.trim() == "drop_dentry_cache" {
            crate::fs::dentry_cache::clean_dentry_cache();
            crate::fs::ext4::clean_inode_cache();
            return Ok(buf.len());
        }
        if command.trim() != "reset" {
            return Err(Errno::EINVAL);
        }
        crate::perf::reset();
        Ok(buf.len())
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
