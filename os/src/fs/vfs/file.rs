// os/src/vfs/file.rs

use spin::Mutex;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use crate::syscall::{Errno, SysResult};
use crate::fs::KStat;
use super::{InodeOp, DirEntry};

// 常规文件
pub struct File {
    inode: Arc<dyn InodeOp>,
    inner: Mutex<FileInner>,
}

struct FileInner {
    offset: usize,
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
}

impl File {
    pub fn new(inode: Arc<dyn InodeOp>, flags: OpenFlags) -> Self {
        Self {
            inode,
            inner: Mutex::new(FileInner {
                offset: 0,
                flags,
            }),
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let n = self.inode.read_at(inner.offset, buf)?;
        inner.offset += n;
        Ok(n)
    }

    pub fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut inner = self.inner.lock();
        let n = self.inode.write_at(inner.offset, buf)?;
        inner.offset += n;
        Ok(n)
    }

    pub fn seek(&self, offset: usize) {
        self.inner.lock().offset = offset;
    }

    pub fn offset(&self) -> usize {
        self.inner.lock().offset
    }

    pub fn readdir(&self) -> SysResult<Vec<DirEntry>> {
        self.inode.readdir()
    }

    pub fn stat(&self) -> SysResult<KStat> {
        self.inode.stat()
    }

    pub fn inode(&self) -> Arc<dyn InodeOp> {
        self.inode.clone()
    }
}

impl File {
    pub fn read_all(&self) -> SysResult<Vec<u8>> {
        let stat = self.stat()?;
        let size = stat.size;

        let mut data = alloc::vec![0u8; size];
        let mut offset = 0;

        while offset < size {
            let n = self.inode.read_at(offset, &mut data[offset..])?;
            if n == 0 {
                break;
            }
            offset += n;
        }

        data.truncate(offset);
        Ok(data)
    }
}

impl FileOp for File {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        self.read(buf)
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        self.write(buf)
    }

    fn seek(&self, offset: isize) -> SysResult<usize> {
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        self.seek(offset);
        Ok(offset)
    }

    fn get_offset(&self) -> usize {
        self.offset()
    }

    fn get_flags(&self) -> OpenFlags {
        self.inner.lock().flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        self.stat()
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
        const O_TRUNC  = 1 << 9;
        const O_APPEND = 1 << 10;
        const O_DIRECTORY = 1 << 16;
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
