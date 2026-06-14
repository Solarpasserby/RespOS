use super::super::KStat;
use super::super::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
use super::{DEVFS_DEV, SHM_DIR_INO};
use crate::syscall::{Errno, SysResult};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;

const SHM_FILE_INO_BASE: u64 = 0x1000;

lazy_static! {
    static ref SHM_DIR: Arc<ShmDirInode> = Arc::new(ShmDirInode::new(SHM_DIR_INO, 0o777));
}

static NEXT_SHM_INO: AtomicU64 = AtomicU64::new(SHM_FILE_INO_BASE);

pub(super) fn shm_dir() -> Arc<dyn InodeOp> {
    SHM_DIR.clone()
}

struct ShmDirInode {
    ino: u64,
    mode: Mutex<u32>,
    entries: Mutex<BTreeMap<String, Arc<dyn InodeOp>>>,
}

impl ShmDirInode {
    fn new(ino: u64, mode: u32) -> Self {
        Self {
            ino,
            mode: Mutex::new(mode),
            entries: Mutex::new(BTreeMap::new()),
        }
    }
}

impl InodeOp for ShmDirInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Directory)
            .with_dev(DEVFS_DEV)
            .with_ino(self.ino)
            .with_mode(*self.mode.lock())
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        self.entries
            .lock()
            .get(name)
            .cloned()
            .ok_or(Errno::ENOENT)
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        let children = self.entries.lock();
        let mut entries = vec![dir_entry(self.ino, 1, b".\0"), dir_entry(1, 2, b"..\0")];

        for (idx, (name, inode)) in children.iter().enumerate() {
            let mut d_name = Vec::from(name.as_bytes());
            d_name.push(0);
            let stat = inode.stat("").unwrap_or_else(|_| KStat::minimal(0, inode.node_type()));
            entries.push(entry(
                stat.ino,
                inode.node_type(),
                (idx + 3) as i64,
                d_name.as_slice(),
            ));
        }

        Ok(entries)
    }

    fn read_at(&self, _path: &str, _off: usize, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn write_at(&self, _path: &str, _off: usize, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn truncate(&self, _path: &str, _size: usize) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn create(&self, _parent_path: &str, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>> {
        if name.is_empty() || name.contains('/') {
            return Err(Errno::EINVAL);
        }
        if ty != InodeType::Regular && ty != InodeType::Directory {
            return Err(Errno::EINVAL);
        }

        let mut entries = self.entries.lock();
        if entries.contains_key(name) {
            return Err(Errno::EEXIST);
        }

        let ino = NEXT_SHM_INO.fetch_add(1, Ordering::Relaxed);
        let inode: Arc<dyn InodeOp> = match ty {
            InodeType::Regular => Arc::new(ShmFileInode::new(ino)),
            InodeType::Directory => Arc::new(ShmDirInode::new(ino, 0o777)),
            _ => return Err(Errno::EINVAL),
        };
        entries.insert(name.to_string(), inode.clone());
        Ok(inode)
    }

    fn set_mode(&self, _path: &str, mode: u32) -> SysResult {
        *self.mode.lock() = mode & 0o7777;
        Ok(())
    }

    fn link(&self, _old_path: &str, _bare_dentry: Arc<Dentry>) -> SysResult {
        Err(Errno::EPERM)
    }

    fn unlink(&self, valid_dentry: &Arc<Dentry>) -> SysResult {
        let name = valid_dentry
            .abs_path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or(Errno::ENOENT)?;

        let mut entries = self.entries.lock();
        let inode = entries.get(name).cloned().ok_or(Errno::ENOENT)?;
        if let Some(dir) = inode.as_any().downcast_ref::<ShmDirInode>() {
            if !dir.entries.lock().is_empty() {
                return Err(Errno::ENOTEMPTY);
            }
        }
        entries.remove(name);
        Ok(())
    }
}

struct ShmFileInode {
    ino: u64,
    mode: Mutex<u32>,
    data: Mutex<Vec<u8>>,
}

impl ShmFileInode {
    fn new(ino: u64) -> Self {
        Self {
            ino,
            mode: Mutex::new(0o666),
            data: Mutex::new(Vec::new()),
        }
    }
}

impl InodeOp for ShmFileInode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::Regular
    }

    fn stat(&self, _path: &str) -> SysResult<KStat> {
        let size = self.data.lock().len();
        Ok(KStat::minimal(size, InodeType::Regular)
            .with_dev(DEVFS_DEV)
            .with_ino(self.ino)
            .with_mode(*self.mode.lock()))
    }

    fn read_at(&self, _path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        let data = self.data.lock();
        if off >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - off);
        buf[..n].copy_from_slice(&data[off..off + n]);
        Ok(n)
    }

    fn write_at(&self, _path: &str, off: usize, buf: &[u8]) -> SysResult<usize> {
        let end = off.checked_add(buf.len()).ok_or(Errno::EINVAL)?;
        let mut data = self.data.lock();
        if data.len() < end {
            data.resize(end, 0);
        }
        data[off..end].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn truncate(&self, _path: &str, size: usize) -> SysResult<usize> {
        self.data.lock().resize(size, 0);
        Ok(0)
    }

    fn set_mode(&self, _path: &str, mode: u32) -> SysResult {
        *self.mode.lock() = mode & 0o7777;
        Ok(())
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
        Err(Errno::ENOTDIR)
    }

    fn link(&self, _old_path: &str, _bare_dentry: Arc<Dentry>) -> SysResult {
        Err(Errno::EPERM)
    }

    fn unlink(&self, _valid_dentry: &Arc<Dentry>) -> SysResult {
        Err(Errno::ENOTDIR)
    }
}

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
