// os/src/fs/fdtable.rs

use alloc::{vec, vec::Vec};
use alloc::sync::Arc;
use crate::syscall::{SysResult, Errno};
use super::vfs::{FileOp, OpenFlags};
use super::{Stdin, Stdout};

pub struct FdTable {
    pub table: Vec<Option<Fd>>
}

impl FdTable {
    pub fn new() -> Self {
        // 自带三个文件描述符，分别是标准输入、标准输出、标准错误
        let stdin  = Fd::new(Arc::new(Stdin), OpenFlags::O_RDONLY);
        let stdout = Fd::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let stderr = Fd::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        FdTable {
            table: vec![Some(stdin), Some(stdout), Some(stderr)],
        }
    }

    // 找到一个空位分配fd，返回fd的下标就是新fd
    pub fn alloc_fd(&mut self, fd: Fd) -> SysResult<usize> {
        // 先判断是否有没有使用的空闲fd， 用idx作为数组下标
        if let Some(valid_idx) = (0..self.table_len()).find(|idx| self.table[*idx].is_none()) {
            self.put_in(fd, valid_idx)?;
            Ok(valid_idx)
        } else {
            // 在最后加入
            let new_fd = self.table_len();
            self.put_in(fd, new_fd)?;
            Ok(new_fd)
        }
    }

    // 在指定位置加入Fd
    pub fn put_in(&mut self, fd: Fd, idx: usize) -> SysResult {
        if idx > RLIMIT_NOFILE {
            return Err(Errno::EMFILE);
        }
        if idx >= self.table_len() {
            self.table.resize(idx + 1, Fd::new_bare());
        }
        self.table[idx] = fd;
        Ok(())
    }

    pub fn remove(&mut self, fd: usize) -> SysResult {
        if fd >= self.table_len() || self.table[fd].is_none() {
            return Err(Errno::EBADF);
        }
        self.table[fd].clear();
        Ok(())
    }

    pub fn table_len(&self) -> usize {
        self.table.len()
    }

    pub fn get_file(&self, idx: usize) -> SysResult<Arc<dyn FileOp>> {
        if idx >= self.table_len() {
            return  Err(Errno::EBADF);
        }
        Ok(self.table[idx].file.as_ref().map(|fd| fd.clone()))
    }

    pub fn get_fd(&self, idx: usize) -> SysResult<Fd> {
        if idx >= self.table_len() {
            return Err(Errno::EBADF);
        }
        Ok(self.table[idx].clone())
    }
}

pub struct Fd {
    pub file: Arc<dyn FileOp>,
    pub flags: OpenFlags,
}

impl Fd {
    pub fn new(file: Arc<dyn FileOp>, flags: OpenFlags) -> Self {
        Self {
            file,
            flags,
        }
    }

    #[inline(always)]
    pub fn get_file(&self) -> Arc<dyn FileOp> {
        self.file.clone()
    }
    #[inline(always)]
    pub fn get_flags(&self) -> OpenFlags {
        self.flags
    }
    #[inline(always)]
    pub fn set_flags(&mut self, flags: OpenFlags) {
        self.flags = flags;
    }
}
