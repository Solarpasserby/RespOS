// os/src/vfs/file.rs

use spin::Mutex;
use alloc::sync::Arc;
use core::any::Any;
use crate::syscall::SysResult;
use super::InodeOp;

// 常规文件
pub struct File {
    inner: Mutex<FileInner>,
}

pub struct FileInner {
    /// 单位是字节
    offset: usize,
    // pub dentry: Arc<Dentry>,
    pub path: Arc<Path>,
    pub inode: Arc<dyn InodeOp>,
    pub flags: usize,
}

/// 文件操作 trait
pub trait FileOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    /// 读取数据到 buf 中，返回读取的字节数，同时更新文件偏移量
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize>;
    /// 写入数据到 buf 中，返回写入的字节数，同时更新文件偏移量
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize>;
    // move the file offset
    fn seek(&self, offset: usize) -> SysResult<usize>;
    // Get the file offset
    fn get_offset(&self) -> usize;
    // readable
    fn readable(&self) -> bool;
    // writable
    fn writable(&self) -> bool;
}

impl File {
    pub fn inner_handler<T>(&self, f: impl FnOnce(&mut FileInner) -> T) -> T {
        f(&mut self.inner.lock())
    }
    pub fn add_offset(&self, offset: usize) {
        self.inner_handler(|inner| inner.offset += offset);
    }
    pub fn get_offset(&self) -> usize {
        self.inner_handler(|inner| inner.offset)
    }
}

impl File {
    pub fn new(path: Arc<Path>, inode: Arc<dyn InodeOp>, flags: usize) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                path,
                inode,
                flags,
            }),
        }
    }

    /// Read all data inside a inode into vector
    pub fn read_all(&self) -> Vec<u8> {
        let inode = self.inner_handler(|inner| inner.inode.clone());
        let mut buffer = [0u8; PAGE_SIZE];
        let mut v: Vec<u8> = Vec::new();
        // Debug
        let mut totol_read = 0;
        loop {
            let offset = self.get_offset();
            let len = inode.read(offset, &mut buffer);
            totol_read += len;
            if len == 0 {
                break;
            }
            self.add_offset(len);
            v.extend_from_slice(&buffer[..len]);
        }
        v
    }
    pub fn is_dir(&self) -> bool {
        self.inner_handler(|inner| inner.inode.can_lookup())
    }

    pub fn readdir(&self) -> Result<Vec<LinuxDirent64>, &'static str> {
        if self.is_dir() {
            return Ok(self.inner_handler(|inner| inner.inode.getdents()));
        }
        return Err("not a directory");
    }
}

impl FileOp for File {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> usize {
        let read_size = self.inner_handler(|inner| inner.inode.read(inner.offset, buf));
        self.add_offset(read_size);
        read_size
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> usize {
        let write_size = self.inner_handler(|inner| inner.inode.write(inner.offset, buf));
        self.add_offset(write_size);
        write_size
    }
    fn seek(&self, offset: usize) {
        self.inner_handler(|inner| inner.offset = offset);
    }
    fn get_offset(&self) -> usize {
        self.inner_handler(|inner| inner.offset)
    }
    // O_RDONLY = 0, 以只读方式打开文件, 具体的权限检查由VFS层完成
    // Todo:
    fn readable(&self) -> bool {
        // self.inner_handler(|inner| inner.flags & O_RDONLY != 0)
        true
    }
    // Todo:
    fn writable(&self) -> bool {
        true
    }
}
