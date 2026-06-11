// os/src/fs/proc/mounts.rs

use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::super::{KStat, Path};
use super::dirs::{proc_dev, proc_mounts_ino};
use crate::fs::mount::{Mount, get_mount_by_vfsmount, path_global_abs_path, root_path};
use crate::syscall::{Errno, SysResult};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

pub(super) struct MountsInode;

impl InodeOp for MountsInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = generate_mounts().len();
        Ok(KStat::minimal(size, InodeType::Regular)
            .with_dev(proc_dev())
            .with_ino(proc_mounts_ino())
            .with_mode(0o444))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let content = generate_mounts();
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

fn generate_mounts() -> String {
    let mut result = String::new();
    let root = root_path();
    if let Some(root_mount) = get_mount_by_vfsmount(&root.mnt) {
        collect_mount(&root_mount, &mut result);
    }
    result
}

fn collect_mount(mount: &Arc<Mount>, result: &mut String) {
    let dev = "none";
    let target = mount_point_path(mount);
    let fstype = mount_fstype(mount);
    let opts = if mount.vfs_mount.flags & 1 != 0 {
        "ro"
    } else {
        "rw"
    };
    let _ = writeln!(result, "{} {} {} {} 0 0", dev, target, fstype, opts);

    for child in mount.children.lock().iter() {
        collect_mount(child, result);
    }
}

fn mount_fstype(mount: &Arc<Mount>) -> &'static str {
    match mount.vfs_mount.fs.statfs().map(|stat| stat.f_type).ok() {
        Some(0xEF53) => "ext4",
        Some(0x9fa0) => "proc",
        Some(0x1373) => "devfs",
        _ => "unknown",
    }
}

fn mount_point_path(mount: &Arc<Mount>) -> String {
    if let Some(parent) = mount.parent.as_ref().and_then(|w| w.upgrade()) {
        let mp = Path::new(parent.vfs_mount.clone(), mount.mountpoint.clone());
        path_global_abs_path(&mp)
    } else {
        "/".into()
    }
}
