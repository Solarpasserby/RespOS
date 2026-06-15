// os/src/fs/fdtable.rs

use super::{FileOp, OpenFlags};
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
    nofile_cur: AtomicUsize,
    nofile_max: AtomicUsize,
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
            nofile_cur: AtomicUsize::new(FTB_RLIMIT),
            nofile_max: AtomicUsize::new(FTB_RLIMIT),
        })
    }

    /// 重置文件描述符表
    pub fn reset(&self) {
        let stdin = FdEntry::new(Arc::new(Stdin), OpenFlags::O_RDONLY);
        let stdout = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let stderr = FdEntry::new(Arc::new(Stdout), OpenFlags::O_WRONLY);
        let old_table = {
            let mut table = self.table.lock();
            core::mem::replace(&mut *table, vec![Some(stdin), Some(stdout), Some(stderr)])
        };
        self.next_fd.store(3, Ordering::Relaxed);
        drop(old_table);
    }

    pub fn nofile_limit(&self) -> (usize, usize) {
        (
            self.nofile_cur.load(Ordering::Relaxed),
            self.nofile_max.load(Ordering::Relaxed),
        )
    }

    pub fn set_nofile_limit(&self, cur: usize, max: usize) -> SysResult {
        if cur > max || max > FTB_RLIMIT {
            return Err(Errno::EINVAL);
        }
        self.nofile_cur.store(cur, Ordering::Relaxed);
        self.nofile_max.store(max, Ordering::Relaxed);
        Ok(())
    }

    // 找到一个空位分配 FdEntry，返回 FdTable 中 Fd 的下标
    pub fn alloc_fd(&self, fd_entry: FdEntry) -> SysResult<usize> {
        let mut table = self.table.lock();
        let min_fd = self.next_fd.load(Ordering::Relaxed);
        let limit = self.nofile_cur.load(Ordering::Relaxed).min(FTB_RLIMIT);
        if min_fd >= limit {
            return Err(Errno::EMFILE);
        }
        Self::alloc_fd_locked(&mut table, fd_entry, min_fd, limit, &self.next_fd)
    }

    /// 从 min_fd 开始找空位分配，用于 F_DUPFD
    pub fn alloc_fd_from(&self, fd_entry: FdEntry, min_fd: usize) -> SysResult<usize> {
        let mut table = self.table.lock();
        let limit = self.nofile_cur.load(Ordering::Relaxed).min(FTB_RLIMIT);
        if min_fd >= limit {
            return Err(Errno::EINVAL);
        }
        Self::alloc_fd_locked(&mut table, fd_entry, min_fd, limit, &self.next_fd)
    }

    fn alloc_fd_locked(
        table: &mut Vec<Option<FdEntry>>,
        fd_entry: FdEntry,
        min_fd: usize,
        limit: usize,
        next_fd: &AtomicUsize,
    ) -> SysResult<usize> {
        // 扩展表以容纳 min_fd
        if min_fd >= table.len() {
            table.resize_with(min_fd + 1, || None);
        }
        if let Some((fd, it)) = table
            .iter_mut()
            .enumerate()
            .skip(min_fd)
            .take(limit.saturating_sub(min_fd))
            .find(|(_, it)| it.is_none())
        {
            *it = Some(fd_entry);
            let nf = Self::update_next_fd(table, fd + 1);
            next_fd.store(nf, Ordering::Relaxed);
            return Ok(fd);
        }
        if table.len() >= limit {
            return Err(Errno::EMFILE);
        }
        let fd = table.len();
        table.push(Some(fd_entry));
        next_fd.store(table.len(), Ordering::Relaxed);
        Ok(fd)
    }

    pub fn set_fd(&self, fd: usize, fd_entry: FdEntry) -> SysResult<Option<FdEntry>> {
        let limit = self.nofile_cur.load(Ordering::Relaxed).min(FTB_RLIMIT);
        if fd >= limit {
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
        let old = {
            let mut table = self.table.lock();
            if fd >= table.len() {
                return Err(Errno::EBADF);
            }
            let old = table[fd].take().ok_or(Errno::EBADF)?;
            let next_fd = self.next_fd.load(Ordering::Relaxed).min(fd);
            self.next_fd.store(next_fd, Ordering::Relaxed);
            old
        };
        old.file.fsync()?;
        drop(old);
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

    pub fn open_fds(&self) -> Vec<usize> {
        self.table
            .lock()
            .iter()
            .enumerate()
            .filter_map(|(fd, entry)| entry.as_ref().map(|_| fd))
            .collect()
    }

    pub fn close_on_exec(&self) {
        let old_entries = {
            let mut table = self.table.lock();
            let mut old_entries = Vec::new();
            for entry in table.iter_mut() {
                if entry
                    .as_ref()
                    .is_some_and(|entry| entry.flags.contains(OpenFlags::O_CLOEXEC))
                {
                    old_entries.push(entry.take().unwrap());
                }
            }
            let next_fd = Self::update_next_fd(&table, 0);
            self.next_fd.store(next_fd, Ordering::Relaxed);
            old_entries
        };
        drop(old_entries);
    }

    /// 清空文件描述符表
    pub fn clear(&self) {
        let old_table = {
            let mut table = self.table.lock();
            core::mem::take(&mut *table)
        };
        self.next_fd.store(0, Ordering::Relaxed);
        drop(old_table);
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
            nofile_cur: AtomicUsize::new(fd_table.nofile_cur.load(Ordering::Relaxed)),
            nofile_max: AtomicUsize::new(fd_table.nofile_max.load(Ordering::Relaxed)),
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
