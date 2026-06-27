use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::dirs::{proc_dev, proc_health_ino};
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::{self, Write};

pub(super) struct HealthInode;

struct HealthBuffer {
    bytes: [u8; 256],
    len: usize,
}

impl HealthBuffer {
    fn new() -> Self {
        Self {
            bytes: [0; 256],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for HealthBuffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        let dst = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        dst.copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl InodeOp for HealthInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_health_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_health().ok_or(Errno::EAGAIN)?;
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

fn generate_health() -> Option<HealthBuffer> {
    let (ready, blocked, deferred) = crate::task::scheduler_health_counts()?;
    let free_kb = crate::mm::try_free_frame_count()? * crate::config::PAGE_SIZE / 1024;
    let heap_kb = crate::mm::try_heap_allocated()? / 1024;
    let tasks = crate::task::TASK_MANAGER.try_len()?;
    let mut result = HealthBuffer::new();
    writeln!(
        result,
        "free_kb={} cached_kb={} heap_kb={} tasks={} ready={} blocked={} deferred={}",
        free_kb,
        crate::fs::page_cache_page_count() * crate::config::PAGE_SIZE / 1024,
        heap_kb,
        tasks,
        ready,
        blocked,
        deferred,
    )
    .ok()?;
    Some(result)
}
