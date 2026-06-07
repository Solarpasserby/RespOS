// os/src/vfs/file.rs

use super::{InodeOp, InodeType, LinuxDirent64};
use crate::config::PAGE_SIZE;
use crate::fs::{KStat, Path};
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

// 常规文件
pub struct File {
    inode: Arc<dyn InodeOp>,
    inner: Mutex<FileInner>,
}

struct FileInner {
    offset: usize,
    path: Arc<Path>,
    flags: OpenFlags,
    cache: Option<FileCache>,
    write_back: bool,
}

struct FileCache {
    len: usize,
    pages: Vec<Option<Vec<u8>>>,
}

impl FileCache {
    fn new(len: usize) -> Self {
        let page_count = len.div_ceil(PAGE_SIZE);
        Self {
            len,
            pages: (0..page_count).map(|_| None).collect(),
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn ensure_page_slots(&mut self, len: usize) {
        let page_count = len.div_ceil(PAGE_SIZE);
        while self.pages.len() < page_count {
            self.pages.push(None);
        }
    }

    fn extend_len(&mut self, len: usize) {
        self.ensure_page_slots(len);
        self.len = self.len.max(len);
    }

    fn ensure_page_loaded(
        &mut self,
        page_idx: usize,
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
    ) -> SysResult<()> {
        if self
            .pages
            .get(page_idx)
            .and_then(|page| page.as_ref())
            .is_some()
        {
            return Ok(());
        }

        self.ensure_page_slots((page_idx + 1) * PAGE_SIZE);
        let mut page = alloc::vec![0u8; PAGE_SIZE];
        let page_start = page_idx * PAGE_SIZE;
        if page_start < self.len
            && let Some((inode, path)) = lower
        {
            match inode.read_at(path, page_start, &mut page) {
                Ok(_) | Err(Errno::ENOENT) => {}
                Err(err) => return Err(err),
            }
        }
        self.pages[page_idx] = Some(page);
        Ok(())
    }

    fn read_at(
        &mut self,
        offset: usize,
        buf: &mut [u8],
        lower: Option<(&Arc<dyn InodeOp>, &str)>,
    ) -> SysResult<usize> {
        let mut copied = 0;
        let mut pos = offset.min(self.len);
        let end = self.len.min(offset.saturating_add(buf.len()));
        while pos < end {
            let page_idx = pos / PAGE_SIZE;
            let page_off = pos % PAGE_SIZE;
            let n = (end - pos).min(PAGE_SIZE - page_off);
            self.ensure_page_loaded(page_idx, lower)?;
            let page = self.pages[page_idx].as_ref().unwrap();
            buf[copied..copied + n].copy_from_slice(&page[page_off..page_off + n]);
            pos += n;
            copied += n;
        }
        Ok(copied)
    }

    fn write_at(&mut self, offset: usize, buf: &[u8]) -> SysResult<usize> {
        let end = offset.checked_add(buf.len()).ok_or(Errno::EINVAL)?;
        self.extend_len(end);

        let mut copied = 0;
        let mut pos = offset;
        while copied < buf.len() {
            let page_idx = pos / PAGE_SIZE;
            let page_off = pos % PAGE_SIZE;
            let n = (buf.len() - copied).min(PAGE_SIZE - page_off);
            if self.pages[page_idx].is_none() {
                self.pages[page_idx] = Some(alloc::vec![0u8; PAGE_SIZE]);
            }
            let page = self.pages[page_idx].as_mut().unwrap();
            page[page_off..page_off + n].copy_from_slice(&buf[copied..copied + n]);
            pos += n;
            copied += n;
        }
        Ok(buf.len())
    }
}

/// 文件操作 trait
pub trait FileOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    /// 读取数据到 buf 中，返回读取的字节数，同时更新文件偏移量
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize>;
    /// 写入数据到 buf 中，返回写入的字节数，同时更新文件偏移量
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize>;
    // 移动文件偏移
    fn seek(&self, offset: isize) -> SysResult<usize>;
    // 获得文件偏移
    fn get_offset(&self) -> usize;
    // 获得文件打开标志
    fn get_flags(&self) -> OpenFlags;
    fn get_stat(&self) -> SysResult<KStat>;
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn is_tty(&self) -> bool {
        false
    }
}

impl File {
    pub fn new(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        let abs_path = path.abs_path();
        let ty = inode.node_type();
        if flags.contains(OpenFlags::O_TRUNC)
            && flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
        {
            let _ = inode.truncate(&abs_path, 0);
        }
        let size = if ty == InodeType::Regular {
            inode.stat(&abs_path).map(|stat| stat.size).unwrap_or(0)
        } else {
            0
        };
        let offset = if flags.contains(OpenFlags::O_APPEND) {
            size
        } else {
            0
        };
        let cache = (ty == InodeType::Regular).then(|| FileCache::new(size));
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset,
                path,
                flags,
                cache,
                write_back: ty == InodeType::Regular,
            }),
        }
    }

    pub fn new_tmpfile(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset: 0,
                path,
                flags,
                cache: Some(FileCache::new(0)),
                write_back: false,
            }),
        }
    }

    pub fn read_all(&self) -> SysResult<Vec<u8>> {
        let mut inner = self.inner.lock();
        let path = inner.path.abs_path();
        let write_back = inner.write_back;

        if let Some(cache) = inner.cache.as_mut() {
            let mut data = alloc::vec![0u8; cache.len()];
            let lower = write_back.then_some((&self.inode, path.as_str()));
            let n = cache.read_at(0, &mut data, lower)?;
            data.truncate(n);
            return Ok(data);
        }

        let size = self.inode.stat(&path)?.size;

        let mut data = alloc::vec![0u8; size];
        let mut offset = 0;

        while offset < size {
            let n = self.inode.read_at(&path, offset, &mut data[offset..])?;
            if n == 0 {
                break;
            }
            offset += n;
        }

        data.truncate(offset);
        Ok(data)
    }

    pub fn readdir(&self) -> SysResult<Vec<LinuxDirent64>> {
        let path = self.path();
        let mut entries = self.inode.readdir(&path.abs_path())?;

        if Arc::ptr_eq(&path.dentry, &path.mnt.root)
            && let Some(mount) = crate::fs::mount::get_mount_by_vfsmount(&path.mnt)
            && let Some(parent_ino) = mount
                .mountpoint
                .get_parent()
                .and_then(|parent| parent.get_inode().stat(&parent.abs_path).ok())
                .map(|stat| stat.ino)
        {
            for entry in &mut entries {
                if entry.d_name == b"..\0" {
                    entry.d_ino = parent_ino;
                    break;
                }
            }
        }

        Ok(entries)
    }
}

impl File {
    pub fn inode(&self) -> Arc<dyn InodeOp> {
        self.inode.clone()
    }

    pub fn path(&self) -> Arc<Path> {
        self.inner.lock().path.clone()
    }
}

impl FileOp for File {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let path = inner.path.abs_path();
        let offset = inner.offset;
        let lower = inner.write_back.then_some((&self.inode, path.as_str()));
        let n = if let Some(cache) = inner.cache.as_mut() {
            cache.read_at(offset, buf, lower)?
        } else {
            self.inode.read_at(&path, offset, buf)?
        };
        inner.offset += n;
        Ok(n)
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let path = inner.path.abs_path();
        if inner.flags.contains(OpenFlags::O_APPEND) {
            if let Some(cache) = inner.cache.as_ref() {
                inner.offset = cache.len();
            } else {
                inner.offset = self.inode.stat(&path)?.size;
            }
        }

        let offset = inner.offset;
        let write_back = inner.write_back;
        let n = if let Some(cache) = inner.cache.as_mut() {
            let n = cache.write_at(offset, buf)?;
            if write_back {
                match self.inode.write_at(&path, offset, buf) {
                    Ok(_) | Err(Errno::ENOENT) => {}
                    Err(err) => return Err(err),
                }
            }
            n
        } else {
            self.inode.write_at(&path, offset, buf)?
        };
        inner.offset += n;
        Ok(n)
    }

    fn seek(&self, offset: isize) -> SysResult<usize> {
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        self.inner.lock().offset = offset;
        Ok(offset)
    }

    fn get_offset(&self) -> usize {
        self.inner.lock().offset
    }

    fn get_flags(&self) -> OpenFlags {
        self.inner.lock().flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        let inner = self.inner.lock();
        let path = inner.path.abs_path();
        if let Some(cache) = inner.cache.as_ref() {
            let mut stat = match self.inode.stat(&path) {
                Ok(stat) => stat,
                Err(Errno::ENOENT) => KStat::minimal(0, InodeType::Regular),
                Err(err) => return Err(err),
            };
            stat.size = cache.len();
            stat.blocks = KStat::blocks_for_size(stat.size as u64);
            return Ok(stat);
        }
        self.inode.stat(&path)
    }

    fn readable(&self) -> bool {
        !self.get_flags().contains(OpenFlags::O_WRONLY)
    }

    fn writable(&self) -> bool {
        self.get_flags()
            .intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    }
}

bitflags::bitflags! {
    pub struct OpenFlags: u32 {
        const O_RDONLY = 0;
        const O_WRONLY = 1 << 0;
        const O_RDWR   = 1 << 1;
        const O_CREATE = 1 << 6;
        const O_EXCL   = 1 << 7;
        const O_TRUNC  = 1 << 9;
        const O_APPEND = 1 << 10;
        const O_DIRECTORY = 1 << 16;
        const O_CLOEXEC = 1 << 19;
        const O_TMPFILE = 0x410000;
    }
}

impl From<usize> for OpenFlags {
    fn from(bits: usize) -> Self {
        Self::from_bits_truncate(bits as u32)
    }
}
impl From<OpenFlags> for usize {
    fn from(flags: OpenFlags) -> Self {
        flags.bits() as usize
    }
}
