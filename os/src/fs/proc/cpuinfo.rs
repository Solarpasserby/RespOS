use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::dirs::{proc_cpuinfo_ino, proc_dev};
use crate::syscall::{Errno, SysResult};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

pub(super) struct CpuinfoInode;

impl InodeOp for CpuinfoInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let content = generate_cpuinfo();
        Ok(KStat::minimal(content.len(), InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_cpuinfo_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_cpuinfo();
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

fn generate_cpuinfo() -> String {
    // LTP 的虚拟化探测会读取 /proc/cpuinfo。这里不伪装完整 Linux 输出，
    // 只提供稳定、可解析的架构基础字段，避免测试初始化因 ENOENT 失败。
    let mut result = String::new();
    #[cfg(target_arch = "riscv64")]
    {
        let _ = writeln!(result, "processor\t: 0");
        let _ = writeln!(result, "hart\t\t: 0");
        let _ = writeln!(result, "isa\t\t: rv64imafdch");
        let _ = writeln!(result, "mmu\t\t: sv39");
        let _ = writeln!(result, "uarch\t\t: qemu");
    }
    #[cfg(target_arch = "loongarch64")]
    {
        let _ = writeln!(result, "processor\t: 0");
        let _ = writeln!(result, "cpu family\t: LoongArch");
        let _ = writeln!(result, "model name\t: LoongArch QEMU");
    }
    result.push('\n');
    result
}
