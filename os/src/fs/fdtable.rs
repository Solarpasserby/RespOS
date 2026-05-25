// os/src/fs/fdtable.rs

use super::vfs::{FileOp, OpenFlags};
use super::{Stdin, Stdout};
use crate::config::FTB_RLIMIT;
use crate::mutex::SpinLock;
use crate::syscall::{Errno, SysResult};
use alloc::sync::Arc;
use alloc::{vec, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

/// 文件描述符表
pub struct FdTable {
    pub table: SpinLock<Vec<Option<FdEntry>>>,
    next_fd: AtomicUsize,
}

impl FdTable {
    /// 创建文件描述符表
    pub fn new() -> Arc<Self> {
        // 自带三个文件描述符，分别是标准输入、标准输出、标准错误
        let stdin = FdEntry::new(Arc::new(Stdin), OpenFlags::O_RDONLY);
        let stdout = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let stderr = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        Arc::new(FdTable {
            table: SpinLock::new(vec![Some(stdin), Some(stdout), Some(stderr)]),
            next_fd: AtomicUsize::new(3),
        })
    }

    /// 重置文件描述符表
    pub fn reset(&self) {
        let stdin = FdEntry::new(Arc::new(Stdin), OpenFlags::O_RDONLY);
        let stdout = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let stderr = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let mut table = self.table.lock();
        *table = vec![Some(stdin), Some(stdout), Some(stderr)];
        self.next_fd.store(3, Ordering::Relaxed);
    }

    // 找到一个空位分配 FdEntry，返回 FdTable 中 Fd 的下标
    pub fn alloc_fd(&self, fd_entry: FdEntry) -> SysResult<usize> {
        let mut table = self.table.lock();
        let next_fd = self.next_fd.load(Ordering::Relaxed);

        if let Some((fd, it)) = table
            .iter_mut()
            .enumerate()
            .skip(next_fd)
            .find(|(_, it)| it.is_none())
        {
            *it = Some(fd_entry);
            let next_fd = Self::update_next_fd(&table, fd + 1);
            self.next_fd.store(next_fd, Ordering::Relaxed);
            return Ok(fd);
        }
        if table.len() >= FTB_RLIMIT {
            return Err(Errno::EMFILE);
        }

        let fd = table.len();
        table.push(Some(fd_entry));
        self.next_fd.store(table.len(), Ordering::Relaxed);
        Ok(fd)
    }

    pub fn set_fd(&self, fd: usize, fd_entry: FdEntry) -> SysResult<Option<FdEntry>> {
        if fd >= FTB_RLIMIT {
            return Err(Errno::EBADF);
        }
        let mut table = self.table.lock();
        if fd >= table.len() {
            table.resize_with(fd + 1, || None);
        }
        let old = table[fd].replace(fd_entry);
        let next_fd = self.next_fd.load(Ordering::Relaxed);
        if fd == next_fd {
            let next_fd = Self::update_next_fd(&table, next_fd + 1);
            self.next_fd.store(next_fd, Ordering::Relaxed);
        }
        Ok(old)
    }

    pub fn close(&self, fd: usize) -> SysResult {
        let mut table = self.table.lock();
        if fd >= table.len() {
            return Err(Errno::EBADF);
        }
        table[fd].take().ok_or(Errno::EBADF)?;
        let next_fd = self.next_fd.load(Ordering::Relaxed).min(fd);
        self.next_fd.store(next_fd, Ordering::Relaxed);
        Ok(())
    }

    /// 根据文件描述符找到对应的文件描述项
    pub fn get_fd_entry(&self, fd: usize) -> SysResult<FdEntry> {
        let table = self.table.lock();
        if fd >= table.len() {
            return Err(Errno::EBADF);
        }
        if let Some(fd_entry) = &table[fd] {
            Ok(fd_entry.clone())
        } else {
            Err(Errno::EBADF)
        }
    }

    /// 清空文件描述符表
    pub fn clear(&self) {
        let mut table = self.table.lock();
        table.clear();
        self.next_fd.store(0, Ordering::Relaxed);
    }
}

impl FdTable {
    fn update_next_fd(table: &[Option<FdEntry>], mut next_fd: usize) -> usize {
        while next_fd < table.len() && table[next_fd].is_some() {
            next_fd += 1;
        }
        next_fd
    }

    pub fn from_existed_user(fd_table: &Arc<FdTable>) -> Arc<Self> {
        // TODO: Pipe 对象在进程 fork 时的复制操作存在不同语义，需结合实际来实现
        let table = fd_table.table.lock();
        let next_fd = fd_table.next_fd.load(Ordering::Relaxed);
        Arc::new(Self {
            table: SpinLock::new(table.clone()),
            next_fd: AtomicUsize::new(next_fd),
        })
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
