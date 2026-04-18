// os/src/fs/fdtable.rs

use alloc::{vec, vec::Vec};
use alloc::sync::Arc;
use crate::config::FTB_RLIMIT;
use crate::syscall::{SysResult, Errno};
use super::vfs::{FileOp, OpenFlags};
use super::{Stdin, Stdout};

pub struct FdTable {
    pub table: Vec<Option<FdEntry>>
}   

impl FdTable {
    pub fn new() -> Self {
        // 自带三个文件描述符，分别是标准输入、标准输出、标准错误
        let stdin  = FdEntry::new(Arc::new(Stdin), OpenFlags::O_RDONLY);
        let stdout = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let stderr = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        FdTable {
            table: vec![Some(stdin), Some(stdout), Some(stderr)],
        }
    }

    // 找到一个空位分配 FdEntry，返回 FdTable 中 Fd 的下标
    pub fn alloc_fd(&mut self, fd_entry: FdEntry) -> SysResult<usize> {
        if self.table.len() >= FTB_RLIMIT {
            return Err(Errno::EMFILE) 
        } 

        if let Some((fd, _fd_entry)) = self.table.iter()
            .enumerate()
            .find(|(_fd, it)| it.is_none()) {
            self.table[fd] = Some(fd_entry);
            Ok(fd)
        } else {
            let fd = self.table.len();
            self.table.push(Some(fd_entry));
            Ok(fd)
        }
    }

    pub fn close(&mut self, fd: usize) -> SysResult {
        if fd >= self.table.len() {
            return Err(Errno::EBADF);
        }
        self.table[fd].take().ok_or(Errno::EBADF)?;
        Ok(())
    }

    pub fn get_file(&self, fd: usize) -> SysResult<Arc<dyn FileOp>> {
        if fd >= self.table.len() {
            return  Err(Errno::EBADF);
        }
        if let Some(fd_entry) = &self.table[fd] {
            Ok(fd_entry.file.clone())
        } else {
            Err(Errno::EBADF)
        }
    }
}

impl FdTable {
    pub fn from_existed_user(fd_table: &FdTable) -> Self {
        Self {
            table: fd_table.table.clone(),
        }
    }
}

#[derive(Clone)]
pub struct FdEntry {
    pub file: Arc<dyn FileOp>,
    pub flags: OpenFlags,
}

impl FdEntry {
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
