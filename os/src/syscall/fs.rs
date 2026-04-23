// os/src/syscall/fs.rs

use crate::fs::{FdEntry, Stat, path_open};
use crate::task::{current_task};
use crate::mm::copy_cstr_from_user;
use super::{SysResult, Errno};

/// 系统调用 sys-read
pub fn sys_read(fd: usize, buf: *mut u8, len: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.readable() {
        return Err(Errno::EBADF);
    }

    let ret = file.read(unsafe {
        core::slice::from_raw_parts_mut(buf, len)
    })?;
    Ok(ret)
}

/// 系统调用 sys-write
pub fn sys_write(fd: usize, buf: *mut u8, len: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let ret = file.write(unsafe {
        core::slice::from_raw_parts_mut(buf, len)
    })?;
    Ok(ret)
}

/// 系统调用 sys-open
pub fn sys_open(path: *const u8, flags: usize, mode: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let path = copy_cstr_from_user(path)?;
    let file = path_open(path.as_str(), flags, mode)?;
    let fd = task.alloc_fd(FdEntry::new(file, flags.into()))?;
    Ok(fd)
}

/// 系统调用 sys-close
pub fn sys_close(fd: usize) -> SysResult<usize> {
    let _ = fd;
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-stat
pub fn sys_stat(path: *const u8, stat: *mut Stat) -> SysResult<usize> {
    let _ = (path, stat);
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-fstat
pub fn sys_fstat(fd: usize, stat: *mut Stat) -> SysResult<usize> {
    let _ = (fd, stat);
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-lseek
pub fn sys_lseek(fd: usize, offset: isize, whence: usize) -> SysResult<usize> {
    let _ = (fd, offset, whence);
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-dup
pub fn sys_dup(fd: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd)?;
    task.alloc_fd(fd_entry)
}

pub fn sys_dup2(fd_src: usize, fd_dst: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd_src)?;
    task.set_fd(fd_dst, fd_entry)?;
    Ok(fd_dst)
}

/// 系统调用 sys-mkdir
pub fn sys_mkdir(path: *const u8, mode: u32) -> SysResult<usize> {
    let _ = (path, mode);
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-unlink
pub fn sys_unlink(path: *const u8) -> SysResult<usize> {
    let _ = path;
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-chdir
pub fn sys_chdir(path: *const u8) -> SysResult<usize> {
    let _ = path;
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-getcwd
pub fn sys_getcwd(buf: *mut u8, len: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let cwd = task.cwd().abs_path();
    // if cwd.len() > len {
    //     return Err(Errno::?);
    // }

    Err(Errno::ENOSYS)
}

/// 系统调用 sys-pipe
pub fn sys_pipe(pipefd: *mut u32) -> SysResult<usize> {
    let _ = pipefd;
    Err(Errno::ENOSYS)
}

// TODO: 系统调用对于用户数据的直接读写不安全，需要改进
