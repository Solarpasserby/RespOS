// os/src/vfs/file.rs

use super::{InodeOp, LinuxDirent64};
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

    /// 检查文件是否支持 seek，不支持应返回 ESPIPE
    fn can_seek(&self) -> SysResult<()> {
        unimplemented!();
    }
}

impl File {
    pub fn new(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        let abs_path = path.abs_path();
        if flags.contains(OpenFlags::O_TRUNC)
            && flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
        {
            let _ = inode.truncate(&abs_path, 0);
        }
        let offset = if flags.contains(OpenFlags::O_APPEND) {
            inode.stat(&abs_path).map(|stat| stat.size).unwrap_or(0)
        } else {
            0
        };
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset,
                path,
                flags,
            }),
        }
    }

    pub fn read_all(&self) -> SysResult<Vec<u8>> {
        let stat = self.get_stat()?;
        let size = stat.size;
        let path = self.path().abs_path();

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
        self.inode.readdir(&self.path().abs_path())
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
        let n = self.inode.read_at(&path, inner.offset, buf)?;
        inner.offset += n;
        Ok(n)
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let path = inner.path.abs_path();
        if inner.flags.contains(OpenFlags::O_APPEND) {
            inner.offset = self.inode.stat(&path)?.size;
        }
        let n = self.inode.write_at(&path, inner.offset, buf)?;
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
        self.inode.stat(&self.path().abs_path())
    }

    fn readable(&self) -> bool {
        !self.get_flags().contains(OpenFlags::O_WRONLY)
    }

    fn writable(&self) -> bool {
        self.get_flags()
            .intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    }

    fn can_seek(&self) -> SysResult<()> {
        Ok(())
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
