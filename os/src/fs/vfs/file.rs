// os/src/vfs/file.rs

use spin::Mutex;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use crate::syscall::SysResult;
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