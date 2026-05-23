// os/src/fs/fdtable.rs

use super::vfs::{FileOp, OpenFlags};
use super::{Stdin, Stdout};
use crate::config::FTB_RLIMIT;
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::{vec, vec::Vec};

pub struct FdTable {
    pub table: Vec<Option<FdEntry>>,
    next_fd: usize,
}

impl FdTable {
    pub fn new() -> Self {
        // 自带三个文件描述符，分别是标准输入、标准输出、标准错误
        let stdin = FdEntry::new(Arc::new(Stdin), OpenFlags::O_RDONLY);
        let stdout = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let stderr = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        FdTable {
            table: vec![Some(stdin), Some(stdout), Some(stderr)],
            next_fd: 3,
        }
    }

    // 找到一个空位分配 FdEntry，返回 FdTable 中 Fd 的下标
    pub fn alloc_fd(&mut self, fd_entry: FdEntry) -> SysResult<usize> {
        if let Some((fd, it)) = self
            .table
            .iter_mut()
            .enumerate()
            .skip(self.next_fd)
            .find(|(_, it)| it.is_none())
        {
            *it = Some(fd_entry);
            self.next_fd = fd + 1;
            self.update_next_fd();
            return Ok(fd);
        }
        if self.table.len() >= FTB_RLIMIT {
            return Err(Errno::EMFILE);
        }

        let fd = self.table.len();
        self.table.push(Some(fd_entry));
        self.next_fd = self.table.len();
        Ok(fd)
    }

    pub fn set_fd(&mut self, fd: usize, fd_entry: FdEntry) -> SysResult<Option<FdEntry>> {
        if fd >= FTB_RLIMIT {
            return Err(Errno::EBADF);
        }
        if fd >= self.table.len() {
            self.table.resize_with(fd + 1, || None);
        }
        let old = self.table[fd].replace(fd_entry);
        if fd == self.next_fd {
            self.next_fd += 1;
            self.update_next_fd();
        }
        Ok(old)
    }

    pub fn close(&mut self, fd: usize) -> SysResult {
        if fd >= self.table.len() {
            return Err(Errno::EBADF);
        }
        self.table[fd].take().ok_or(Errno::EBADF)?;
        self.next_fd = self.next_fd.min(fd);
        Ok(())
    }

    /// 根据文件描述符找到对应的文件描述项
    pub fn get_fd_entry(&self, fd: usize) -> SysResult<FdEntry> {
        if fd >= self.table.len() {
            return Err(Errno::EBADF);
        }
        if let Some(fd_entry) = &self.table[fd] {
            Ok(fd_entry.clone())
        } else {
            Err(Errno::EBADF)
        }
    }
}

impl FdTable {
    fn update_next_fd(&mut self) {
        while self.next_fd < self.table.len() && self.table[self.next_fd].is_some() {
            self.next_fd += 1;
        }
    }

    pub fn from_existed_user(fd_table: &FdTable) -> Self {
        // TODO: Pipe 对象在进程 fork 时的复制操作存在不同语义，需结合实际来实现
        Self {
            table: fd_table.table.clone(),
            next_fd: fd_table.next_fd,
        }
    }
}

/// 文件描述项
#[derive(Clone)]
pub struct FdEntry {
    pub file: Arc<dyn FileOp>,
    pub flags: OpenFlags,
}

impl FdEntry {
    pub fn new(file: Arc<dyn FileOp>, flags: OpenFlags) -> Self {
        Self { file, flags }
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
