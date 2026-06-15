// os/src/vfs/file.rs

use super::vfs::{InodeOp, InodeType, LinuxDirent64};
use crate::fs::page_cache::PageCache;
use crate::fs::{KStat, Path};
use crate::syscall::{Errno, SysResult};
use crate::timer::{TimeSpec, get_time_ms};
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
    /// 普通文件共享 inode 上的页缓存；tmpfile 使用独立页缓存。
    page_cache: Option<Arc<PageCache>>,
    write_back: bool,
}

/// 文件操作 trait
pub trait FileOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    /// 读取数据到 buf 中，返回读取的字节数，同时更新文件偏移量
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize>;
    /// 写入数据到 buf 中，返回写入的字节数，同时更新文件偏移量
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize>;
    /// 检查文件对象是否支持偏移移动。
    fn can_seek(&self) -> SysResult;
    // 移动文件偏移
    fn seek(&self, offset: isize) -> SysResult<usize>;
    // 获得文件偏移
    fn get_offset(&self) -> usize;
    // 获得文件打开标志
    fn get_flags(&self) -> OpenFlags;
    fn get_stat(&self) -> SysResult<KStat>;
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    // 非阻塞可读：数据是否立即可用—— pipe 非空 / 文件总是可读
    fn read_ready(&self) -> bool {
        true
    }
    // 非阻塞可写：是否立即可写—— pipe 非满 / 文件总是可写
    fn write_ready(&self) -> bool {
        true
    }
    fn is_tty(&self) -> bool {
        false
    }
    /// 将文件缓冲数据刷入存储介质。当前文件系统在内存中，默认无操作。
    fn fsync(&self) -> SysResult<usize> {
        Ok(0)
    }
}

impl File {
    pub fn new(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        let abs_path = path.abs_path();
        let ty = inode.node_type();
        let page_cache = inode.get_page_cache();
        if flags.contains(OpenFlags::O_TRUNC)
            && flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
        {
            let _ = inode.truncate(&abs_path, 0);
            if let Some(ref pc) = page_cache {
                pc.resize(0);
            }
        }
        let offset = if flags.contains(OpenFlags::O_APPEND) && ty == InodeType::Regular {
            inode.stat(&abs_path).map(|stat| stat.size).unwrap_or(0)
        } else {
            0
        };
        let write_back = ty == InodeType::Regular && page_cache.is_some();
        if let Some(ref pc) = page_cache {
            let size = inode.stat(&abs_path).map(|stat| stat.size).unwrap_or(0);
            if size > pc.len() {
                pc.resize(size);
            }
        }
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset,
                path,
                flags,
                page_cache,
                write_back,
            }),
        }
    }

    pub fn new_tmpfile(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        let page_cache = Some(PageCache::new(0));
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset: 0,
                path,
                flags,
                page_cache,
                write_back: false,
            }),
        }
    }

    pub fn read_all(&self) -> SysResult<Vec<u8>> {
        let inner = self.inner.lock();
        let path = inner.path.abs_path();

        if let Some(ref pc) = inner.page_cache {
            let size = pc.len();
            let mut data = alloc::vec![0u8; size];
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            let n = pc.read_at(0, &mut data, lower)?;
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

        if Arc::ptr_eq(&path.dentry, &path.mnt.root) {
            if let Some(mount) = crate::fs::mount::get_mount_by_vfsmount(&path.mnt) {
                if let Some(parent_ino) = mount
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

    pub fn truncate(&self, size: usize) -> SysResult<usize> {
        let inner = self.inner.lock();
        let path = inner.path.abs_path();
        self.inode.truncate(&path, size)?;
        if let Some(ref pc) = inner.page_cache {
            pc.resize(size);
            if inner.write_back {
                pc.sync(&self.inode, &path)?;
            }
        }
        if inner.offset > size {
            drop(inner);
            self.inner.lock().offset = size;
        }
        Ok(0)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = <Self as FileOp>::fsync(self);
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
        let n = if let Some(ref pc) = inner.page_cache {
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            pc.read_at(offset, buf, lower)?
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
            let append_off = if let Some(ref pc) = inner.page_cache {
                pc.len()
            } else {
                self.inode.stat(&path)?.size
            };
            inner.offset = append_off;
        }

        let offset = inner.offset;
        let n = if let Some(ref pc) = inner.page_cache {
            let lower = inner.write_back.then_some((&self.inode, path.as_str()));
            let n = pc.write_at(offset, buf, lower)?;
            let end = offset.checked_add(n).ok_or(Errno::EINVAL)?;
            if end > pc.len() {
                pc.resize(end);
            }
            n
        } else {
            self.inode.write_at(&path, offset, buf)?
        };
        if n != 0 && inner.write_back {
            let ms = get_time_ms();
            let now = TimeSpec {
                sec: (ms / 1000) as isize,
                nsec: ((ms % 1000) * 1_000_000) as isize,
            };
            let _ = self.inode.set_times(&path, None, Some(now));
        }
        inner.offset += n;
        Ok(n)
    }

    fn seek(&self, offset: isize) -> SysResult<usize> {
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        self.inner.lock().offset = offset;
        Ok(offset)
    }

    fn can_seek(&self) -> SysResult {
        let ty = self.get_stat()?.ty;
        if ty == InodeType::Regular || ty == InodeType::Directory {
            Ok(())
        } else {
            Err(Errno::ESPIPE)
        }
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
        if let Some(ref pc) = inner.page_cache {
            let mut stat = match self.inode.stat(&path) {
                Ok(stat) => stat,
                Err(Errno::ENOENT) => KStat::minimal(0, InodeType::Regular),
                Err(err) => return Err(err),
            };
            stat.size = pc.len();
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

    fn fsync(&self) -> SysResult<usize> {
        let inner = self.inner.lock();
        if let Some(ref pc) = inner.page_cache {
            if inner.write_back {
                let path = inner.path.abs_path();
                pc.sync(&self.inode, &path)?;
            }
        }
        Ok(0)
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
        const O_NONBLOCK = 1 << 11;
        const O_DIRECT = 1 << 14;
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
