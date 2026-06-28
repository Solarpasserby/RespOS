// os/src/fs/dev/mod.rs

//! 虚拟 devfs 设备文件系统。
//!
//! - `null`  — `/dev/null`，丢弃写入，读取始终返回 EOF
//! - `zero`  — `/dev/zero`，读取返回零字节，写入丢弃

mod cpu_dma_latency;
mod loop_device;
mod null;
mod random;
mod rtc;
mod shm;
mod zero;

const DEVFS_DEV: u64 = 0x400;
const DEVFS_SUPER_MAGIC: i64 = 0x1373;
const DEV_DIR_INO: u64 = 1;
const NULL_INO: u64 = 2;
const ZERO_INO: u64 = 3;
const SHM_DIR_INO: u64 = 4;
const MISC_DIR_INO: u64 = 5;
const RTC_INO: u64 = 6;
const LOOP_CONTROL_INO: u64 = 7;
const LOOP0_INO: u64 = 8;
const RANDOM_INO: u64 = 9;
const URANDOM_INO: u64 = 10;
const CPU_DMA_LATENCY_INO: u64 = 11;
const VDA_INO: u64 = 12;
const VDA2_INO: u64 = 13;
const NULL_RDEV: u64 = (1 << 8) | 3;
const ZERO_RDEV: u64 = (1 << 8) | 5;
const RANDOM_RDEV: u64 = (1 << 8) | 8;
const URANDOM_RDEV: u64 = (1 << 8) | 9;
const CPU_DMA_LATENCY_RDEV: u64 = (10 << 8) | 62;
const RTC_RDEV: u64 = (254 << 8) | 0;
const LOOP_CONTROL_RDEV: u64 = (10 << 8) | 237;
const LOOP0_RDEV: u64 = 7 << 8;
const VDA_RDEV: u64 = 253 << 8;
const VDA2_RDEV: u64 = (253 << 8) | 2;

use super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64, SuperBlockOp};
use super::{KStat, Statfs64};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use cpu_dma_latency::CpuDmaLatencyInode;
pub use loop_device::{LoopControlInode, LoopInode};
use null::NullInode;
use random::RandomInode;
use rtc::RtcInode;
use shm::shm_dir;
use zero::ZeroInode;

use crate::fs::dentry_cache;
use crate::fs::mount::{self, Mount, VfsMount, get_mount_by_dentry};
use crate::syscall::{Errno, SysResult};

// ── /dev ─────────────────────────────────────────────────────────────

struct DevDirInode;

impl InodeOp for DevDirInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(DEVFS_DEV)
            .with_ino(DEV_DIR_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        match name {
            "null" => Ok(Arc::new(NullInode)),
            "zero" => Ok(Arc::new(ZeroInode)),
            "random" => Ok(Arc::new(RandomInode::random())),
            "urandom" => Ok(Arc::new(RandomInode::urandom())),
            "cpu_dma_latency" => Ok(Arc::new(CpuDmaLatencyInode)),
            "shm" => Ok(shm_dir()),
            "misc" => Ok(Arc::new(MiscDirInode)),
            "loop-control" => Ok(Arc::new(LoopControlInode)),
            "loop0" => Ok(Arc::new(LoopInode::new(0))),
            "vda" => Ok(Arc::new(VirtBlkInode::new(VDA_INO, VDA_RDEV))),
            "vda2" => Ok(Arc::new(VirtBlkInode::new(VDA2_INO, VDA2_RDEV))),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(DEV_DIR_INO, 1, b".\0"),
            dir_entry(2, 2, b"..\0"),
            entry(NULL_INO, InodeType::CharDevice, 3, b"null\0"),
            entry(ZERO_INO, InodeType::CharDevice, 4, b"zero\0"),
            entry(RANDOM_INO, InodeType::CharDevice, 5, b"random\0"),
            entry(URANDOM_INO, InodeType::CharDevice, 6, b"urandom\0"),
            entry(
                CPU_DMA_LATENCY_INO,
                InodeType::CharDevice,
                7,
                b"cpu_dma_latency\0",
            ),
            entry(SHM_DIR_INO, InodeType::Directory, 8, b"shm\0"),
            entry(MISC_DIR_INO, InodeType::Directory, 9, b"misc\0"),
            entry(
                LOOP_CONTROL_INO,
                InodeType::CharDevice,
                10,
                b"loop-control\0",
            ),
            entry(LOOP0_INO, InodeType::BlockDevice, 11, b"loop0\0"),
            entry(VDA_INO, InodeType::BlockDevice, 12, b"vda\0"),
            entry(VDA2_INO, InodeType::BlockDevice, 13, b"vda2\0"),
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

struct VirtBlkInode {
    ino: u64,
    rdev: u64,
}

impl VirtBlkInode {
    fn new(ino: u64, rdev: u64) -> Self {
        Self { ino, rdev }
    }
}

impl InodeOp for VirtBlkInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::BlockDevice
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::BlockDevice)
            .with_dev(DEVFS_DEV)
            .with_ino(self.ino)
            .with_mode(0o660)
            .with_rdev(self.rdev))
    }

    fn read_at(&self, _path: &str, _off: usize, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::ENOSYS)
    }

    fn write_at(&self, _path: &str, _off: usize, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::ENOSYS)
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

struct MiscDirInode;

impl InodeOp for MiscDirInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(DEVFS_DEV)
            .with_ino(MISC_DIR_INO)
            .with_mode(0o555)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        match name {
            "rtc" => Ok(Arc::new(RtcInode)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        Ok(vec![
            dir_entry(MISC_DIR_INO, 1, b".\0"),
            dir_entry(DEV_DIR_INO, 2, b"..\0"),
            entry(RTC_INO, InodeType::CharDevice, 3, b"rtc\0"),
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

struct DevSuperBlock;

impl SuperBlockOp for DevSuperBlock {
    fn root_inode(&self) -> Arc<dyn InodeOp> {
        Arc::new(DevDirInode)
    }

    fn sync(&self) {}

    fn statfs(&self) -> SysResult<Statfs64> {
        Ok(Statfs64 {
            f_type: DEVFS_SUPER_MAGIC,
            f_bsize: crate::config::PAGE_SIZE as i64,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 13,
            f_ffree: 0,
            f_namelen: 255,
            f_frsize: crate::config::PAGE_SIZE as i64,
            ..Default::default()
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────

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

// ── init ──────────────────────────────────────────────────────────────

/// 在根文件系统中挂载 devfs，提供最小字符设备目录树。
pub fn init_devfs(root: Arc<Dentry>) {
    let dev_mountpoint = Arc::new(Dentry::new(
        "/dev".into(),
        Some(root.clone()),
        Arc::new(DevDirInode),
    ));
    root.insert_child("dev", dev_mountpoint.clone());
    dentry_cache::insert_dentry_cache(dev_mountpoint.clone());
    dentry_cache::pin_vfs_dentry(dev_mountpoint.clone());

    let dev_root = Arc::new(Dentry::new("/".into(), None, Arc::new(DevDirInode)));
    dentry_cache::pin_vfs_dentry(dev_root.clone());
    let dev_mount = VfsMount::new(dev_root, Arc::new(DevSuperBlock), 0);
    let parent_mount = get_mount_by_dentry(&root).expect("[devfs] root mount is not initialized");
    mount::add_mount(Mount::new_child(dev_mountpoint, dev_mount, parent_mount));
}
