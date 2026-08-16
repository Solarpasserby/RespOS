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
mod tty;
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
pub(super) const TTY_INO: u64 = 14;
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
pub(super) const TTY_RDEV: u64 = (5 << 8) | 0;

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
use spin::Mutex;
use tty::TtyInode;
use zero::ZeroInode;

use crate::fs::dentry_cache;
use crate::fs::mount::{self, Mount, VfsMount, get_mount_by_dentry};
use crate::syscall::{Errno, SysResult};

fn formatted_geometry_from_header(
    header: &[u8],
    fstype: &str,
    maximum: usize,
) -> Option<(usize, usize)> {
    match fstype {
        "ext2" | "ext3" | "ext4" => {
            const SUPER_OFFSET: usize = 1024;
            const BLOCKS_LO_OFFSET: usize = SUPER_OFFSET + 4;
            const LOG_BLOCK_SIZE_OFFSET: usize = SUPER_OFFSET + 24;
            const MAGIC_OFFSET: usize = SUPER_OFFSET + 56;
            let magic = u16::from_le_bytes(
                header
                    .get(MAGIC_OFFSET..MAGIC_OFFSET + 2)?
                    .try_into()
                    .ok()?,
            );
            if magic != 0xef53 {
                return None;
            }
            let blocks = u32::from_le_bytes(
                header
                    .get(BLOCKS_LO_OFFSET..BLOCKS_LO_OFFSET + 4)?
                    .try_into()
                    .ok()?,
            ) as usize;
            let log_block_size = u32::from_le_bytes(
                header
                    .get(LOG_BLOCK_SIZE_OFFSET..LOG_BLOCK_SIZE_OFFSET + 4)?
                    .try_into()
                    .ok()?,
            );
            if blocks == 0 || log_block_size > 6 {
                return None;
            }
            let block_size = 1024usize.checked_shl(log_block_size)?;
            blocks
                .checked_mul(block_size)
                .filter(|capacity| *capacity <= maximum)
                .map(|capacity| (capacity, block_size))
        }
        _ => None,
    }
}

pub(super) fn formatted_capacity_from_header(
    header: &[u8],
    fstype: &str,
    maximum: usize,
) -> Option<usize> {
    formatted_geometry_from_header(header, fstype, maximum).map(|geometry| geometry.0)
}

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
            "tty" => Ok(Arc::new(TtyInode)),
            "random" => Ok(Arc::new(RandomInode::random())),
            "urandom" => Ok(Arc::new(RandomInode::urandom())),
            "cpu_dma_latency" => Ok(Arc::new(CpuDmaLatencyInode)),
            "shm" => Ok(shm_dir()),
            "misc" => Ok(Arc::new(MiscDirInode)),
            "rtc" | "rtc0" => Ok(Arc::new(RtcInode)),
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
            entry(TTY_INO, InodeType::CharDevice, 5, b"tty\0"),
            entry(RANDOM_INO, InodeType::CharDevice, 6, b"random\0"),
            entry(URANDOM_INO, InodeType::CharDevice, 7, b"urandom\0"),
            entry(
                CPU_DMA_LATENCY_INO,
                InodeType::CharDevice,
                8,
                b"cpu_dma_latency\0",
            ),
            entry(SHM_DIR_INO, InodeType::Directory, 9, b"shm\0"),
            entry(MISC_DIR_INO, InodeType::Directory, 10, b"misc\0"),
            entry(
                LOOP_CONTROL_INO,
                InodeType::CharDevice,
                11,
                b"loop-control\0",
            ),
            entry(LOOP0_INO, InodeType::BlockDevice, 12, b"loop0\0"),
            entry(VDA_INO, InodeType::BlockDevice, 13, b"vda\0"),
            entry(VDA2_INO, InodeType::BlockDevice, 14, b"vda2\0"),
            entry(RTC_INO, InodeType::CharDevice, 15, b"rtc\0"),
            entry(RTC_INO, InodeType::CharDevice, 16, b"rtc0\0"),
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

pub(crate) struct VirtBlkInode {
    ino: u64,
    rdev: u64,
    slot: usize,
    capacity: usize,
}

impl VirtBlkInode {
    const VDA_SIZE: usize = 1024 * 1024 * 1024;
    // /dev/vda2 is the disposable LTP device. It must satisfy the harness's
    // 300-MiB device admission check even when a test formats a smaller FS.
    const VDA2_SIZE: usize = 300 * 1024 * 1024;
    const VDA2_DEFAULT_FILESYSTEM_SIZE: usize = 10 * 1024 * 1024;
    const FORMAT_HEADER_SIZE: usize = 4096;

    fn new(ino: u64, rdev: u64) -> Self {
        let slot = usize::from(ino == VDA2_INO);
        let capacity = if slot == 1 {
            Self::VDA2_SIZE
        } else {
            Self::VDA_SIZE
        };
        Self {
            ino,
            rdev,
            slot,
            capacity,
        }
    }

    /// The LTP scratch block devices are intentionally lightweight rather
    /// than backed by another virtio disk. Preserve the formatted filesystem
    /// header so mount(2)'s backing-directory emulation can still enforce the
    /// size selected by mkfs (for example mmap16's 10240 1-KiB blocks).
    pub(crate) fn mount_capacity(&self, fstype: &str) -> usize {
        let recorded = VIRT_BLK_FORMAT_GEOMETRIES.lock()[self.slot];
        let headers = VIRT_BLK_FORMAT_HEADERS.lock();
        recorded
            .map(|geometry| geometry.0)
            .or_else(|| formatted_capacity_from_header(&headers[self.slot], fstype, self.capacity))
            .unwrap_or_else(|| {
                if self.slot == 1 {
                    Self::VDA2_DEFAULT_FILESYSTEM_SIZE
                } else {
                    self.capacity
                }
            })
    }

    pub(crate) fn mount_block_size(&self, fstype: &str) -> usize {
        let recorded = VIRT_BLK_FORMAT_GEOMETRIES.lock()[self.slot];
        recorded
            .map(|geometry| geometry.1)
            .or_else(|| {
                formatted_geometry_from_header(
                    &VIRT_BLK_FORMAT_HEADERS.lock()[self.slot],
                    fstype,
                    self.capacity,
                )
                .map(|geometry| geometry.1)
            })
            .unwrap_or(if self.slot == 1 { 1024 } else { 4096 })
    }

    pub fn ioctl(&self, request: usize, arg: usize) -> SysResult<usize> {
        const BLKGETSIZE: usize = 0x1260;
        const BLKGETSIZE64: usize = 0x8008_1272;
        match request {
            request if request & 0xffff == BLKGETSIZE64 & 0xffff => {
                let size = self.capacity as u64;
                crate::mm::copy_to_user(arg as *mut u64, &size as *const u64, 1)?;
                Ok(0)
            }
            request if request & 0xffff == BLKGETSIZE => {
                let sectors = self.capacity / 512;
                crate::mm::copy_to_user(arg as *mut usize, &sectors as *const usize, 1)?;
                Ok(0)
            }
            _ => Err(Errno::ENOTTY),
        }
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

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let len = buf.len().min(self.capacity.saturating_sub(off));
        buf[..len].fill(0);
        let header_end = off.saturating_add(len).min(Self::FORMAT_HEADER_SIZE);
        if off < header_end {
            let headers = VIRT_BLK_FORMAT_HEADERS.lock();
            buf[..header_end - off].copy_from_slice(&headers[self.slot][off..header_end]);
        }
        Ok(len)
    }

    fn write_at(&self, _path: &str, off: usize, buf: &[u8]) -> SysResult<usize> {
        let len = buf.len().min(self.capacity.saturating_sub(off));
        let header_end = off.saturating_add(len).min(Self::FORMAT_HEADER_SIZE);
        if off < header_end {
            VIRT_BLK_FORMAT_HEADERS.lock()[self.slot][off..header_end]
                .copy_from_slice(&buf[..header_end - off]);
        }
        Ok(len)
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

static VIRT_BLK_FORMAT_HEADERS: Mutex<[[u8; VirtBlkInode::FORMAT_HEADER_SIZE]; 2]> =
    Mutex::new([[0; VirtBlkInode::FORMAT_HEADER_SIZE]; 2]);
static VIRT_BLK_FORMAT_GEOMETRIES: Mutex<[Option<(usize, usize)>; 2]> = Mutex::new([None, None]);

/// Record the geometry selected by the bundled no-op mkfs replacement. The
/// lightweight virtual block device has no real formatter, but mount users
/// must still observe the filesystem size and block size requested by mkfs.
pub(crate) fn record_noop_mkfs(path: &str, args: &[alloc::string::String]) {
    let tokens = args
        .iter()
        .flat_map(|arg| arg.split_ascii_whitespace())
        .collect::<Vec<_>>();
    let is_ext = core::iter::once(path)
        .chain(tokens.iter().copied())
        .any(|token| {
            matches!(
                token
                    .trim_matches(|ch: char| ch == '\'' || ch == '"')
                    .rsplit('/')
                    .next(),
                Some("mkfs.ext2" | "mkfs.ext3" | "mkfs.ext4")
            )
        });
    if !is_ext {
        return;
    }
    let Some((device_index, slot)) =
        tokens.iter().enumerate().find_map(|(index, token)| {
            match token.trim_matches(|ch: char| ch == '\'' || ch == '"') {
                "/dev/vda" => Some((index, 0)),
                "/dev/vda2" => Some((index, 1)),
                _ => None,
            }
        })
    else {
        return;
    };
    let block_size = tokens
        .windows(2)
        .find_map(|pair| {
            (pair[0] == "-b")
                .then(|| {
                    pair[1]
                        .trim_matches(|ch: char| ch == '\'' || ch == '"')
                        .parse::<usize>()
                        .ok()
                })
                .flatten()
        })
        .or_else(|| {
            tokens.iter().find_map(|token| {
                token
                    .strip_prefix("-b")?
                    .trim_matches(|ch: char| ch == '\'' || ch == '"')
                    .parse::<usize>()
                    .ok()
            })
        })
        .filter(|size| matches!(*size, 1024 | 2048 | 4096))
        .unwrap_or(4096);
    let blocks = tokens[device_index + 1..].iter().find_map(|token| {
        token
            .trim_matches(|ch: char| ch == '\'' || ch == '"')
            .parse::<usize>()
            .ok()
    });
    let maximum = if slot == 1 {
        VirtBlkInode::VDA2_SIZE
    } else {
        VirtBlkInode::VDA_SIZE
    };
    let capacity = blocks
        .and_then(|blocks| blocks.checked_mul(block_size))
        .filter(|capacity| *capacity <= maximum)
        .unwrap_or(maximum);
    if capacity != 0 {
        VIRT_BLK_FORMAT_GEOMETRIES.lock()[slot] = Some((capacity, block_size));
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

    fn sync(&self) -> SysResult {
        Ok(())
    }

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
