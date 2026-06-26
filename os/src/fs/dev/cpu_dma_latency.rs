use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::{CPU_DMA_LATENCY_INO, CPU_DMA_LATENCY_RDEV, DEVFS_DEV};
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

/// `/dev/cpu_dma_latency` compatibility node.
///
/// Linux exposes this PM QoS device so userspace can request a maximum CPU DMA
/// latency by keeping the fd open and writing a `s32` latency value. RespOS does
/// not implement CPU idle or PM QoS yet, so this device accepts reads/writes as
/// a harmless no-op. That is enough for latency tools such as cyclictest to
/// avoid warning on startup without changing scheduler semantics.
pub(super) struct CpuDmaLatencyInode;

impl InodeOp for CpuDmaLatencyInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::CharDevice
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::CharDevice)
            .with_dev(DEVFS_DEV)
            .with_ino(CPU_DMA_LATENCY_INO)
            .with_mode(0o666)
            .with_rdev(CPU_DMA_LATENCY_RDEV))
    }

    fn read_at(&self, _path: &str, _off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let value = 0i32.to_ne_bytes();
        let len = buf.len().min(value.len());
        buf[..len].copy_from_slice(&value[..len]);
        Ok(len)
    }

    fn write_at(&self, _path: &str, _off: usize, buf: &[u8]) -> SysResult<usize> {
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
