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
    static ref SHM_DIR: Arc<ShmDirInode> = Arc::new(ShmDirInode::new());
}

static NEXT_SHM_INO: AtomicU64 = AtomicU64::new(SHM_FILE_INO_BASE);

pub(super) fn shm_dir() -> Arc<dyn InodeOp> {
    SHM_DIR.clone()
}

struct ShmDirInode {
    files: Mutex<BTreeMap<String, Arc<ShmFileInode>>>,
}

impl ShmDirInode {
    fn new() -> Self {
        Self {
            files: Mutex::new(BTreeMap::new()),
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
            .with_ino(SHM_DIR_INO)
            .with_mode(0o777)
            .with_nlink(2))
    }

    fn lookup(&self, _parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        self.files
            .lock()
            .get(name)
            .cloned()
            .map(|inode| inode as Arc<dyn InodeOp>)
            .ok_or(Errno::ENOENT)
    }

    fn readdir(&self, _path: &str) -> SysResult<Vec<LinuxDirent64>> {
        let files = self.files.lock();
        let mut entries = vec![dir_entry(SHM_DIR_INO, 1, b".\0"), dir_entry(1, 2, b"..\0")];

        for (idx, (name, inode)) in files.iter().enumerate() {
            let mut d_name = Vec::from(name.as_bytes());
            d_name.push(0);
            entries.push(entry(
                inode.ino,
                InodeType::Regular,
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
        if ty != InodeType::Regular || name.is_empty() || name.contains('/') {
            return Err(Errno::EINVAL);
        }

        let mut files = self.files.lock();
        if files.contains_key(name) {
            return Err(Errno::EEXIST);
        }

        let ino = NEXT_SHM_INO.fetch_add(1, Ordering::Relaxed);
        let file = Arc::new(ShmFileInode::new(ino));
        files.insert(name.to_string(), file.clone());
        Ok(file)
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

        self.files
            .lock()
            .remove(name)
            .map(|_| ())
            .ok_or(Errno::ENOENT)
    }
}

struct ShmFileInode {
    ino: u64,
    data: Mutex<Vec<u8>>,
}

impl ShmFileInode {
    fn new(ino: u64) -> Self {
        Self {
            ino,
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
            .with_mode(0o666))
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
