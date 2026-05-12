// os/src/syscall/fs.rs

use crate::fs::{FdEntry, Path, Stat, filename_create, filename_lookup, make_pipe, path_open};
use crate::fs::vfs::InodeType;
use crate::task::{current_task};
use crate::mm::{check_user_writable, copy_cstr_from_user, copy_from_user, copy_to_user};
use super::{SysResult, Errno};

// 使用 mm 实现的 `copy_cstr_from_user`, `copy_from_user`, `copy_to_user` 来访问用户空间的数据

// TODO: write 和 read 借助堆上分配的空间中转数据，有额外开销，须优化

/// 系统调用 sys-read
pub fn sys_read(fd: usize, buf: *mut u8, len: usize) -> SysResult<usize> {
    if len == 0 {
        return Ok(0);
    }
    check_user_writable(buf, len)?;

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.readable() {
        return Err(Errno::EBADF);
    }

    let mut kbuf = alloc::vec![0u8; len];
    let ret = file.read(kbuf.as_mut_slice())?;
    copy_to_user(buf, kbuf.as_ptr(), ret)?;
    Ok(ret)
}

/// 系统调用 sys-write
pub fn sys_write(fd: usize, buf: *mut u8, len: usize) -> SysResult<usize> {
    if len == 0 {
        return Ok(0);
    }

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let mut kbuf = alloc::vec![0u8; len];
    copy_from_user(kbuf.as_mut_ptr(), buf, len)?;
    let ret = file.write(kbuf.as_slice())?;
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
    let task = current_task().expect("[kernel] current task is None.");
    task.close(fd)?;
    Ok(0)
}

/// 系统调用 sys-stat
pub fn sys_stat(path: *const u8, stat: *mut Stat) -> SysResult<usize> {
    let path = copy_cstr_from_user(path)?;
    let dentry = filename_lookup(path.as_str(), 0)?;
    let stat_buf: Stat = dentry.get_inode().stat()?.into();
    copy_to_user(stat, &stat_buf as *const Stat, 1)?;
    Ok(0)
}

/// 系统调用 sys-fstat
pub fn sys_fstat(fd: usize, stat: *mut Stat) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    let stat_buf: Stat = file.get_stat()?.into();
    copy_to_user(stat, &stat_buf as *const Stat, 1)?;
    Ok(0)
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

/// 系统调用 sys-dup2
pub fn sys_dup2(fd_src: usize, fd_dst: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd_src)?;
    task.set_fd(fd_dst, fd_entry)?;
    Ok(fd_dst)
}

/// 系统调用 sys-mkdir
pub fn sys_mkdir(path: *const u8, mode: usize) -> SysResult<usize> {
    let path = copy_cstr_from_user(path)?;
    filename_create(path.as_str(), InodeType::Directory, mode)?;
    Ok(0)
}

/// 系统调用 sys-unlink
pub fn sys_unlink(path: *const u8) -> SysResult<usize> {
    let _ = path;
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-chdir
pub fn sys_chdir(path: *const u8) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let path = copy_cstr_from_user(path)?;
    let dentry = filename_lookup(path.as_str(), 0)?;
    task.set_cwd(Path::new(dentry));
    Ok(0)
}

/// 系统调用 sys-getcwd
pub fn sys_getcwd(buf: *mut u8, len: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let cwd = task.cwd().abs_path();
    if cwd.len() >= len {
        return Err(Errno::ERANGE);
    }
    let src = cwd.as_bytes().as_ptr();
    copy_to_user(buf, src, cwd.len())?;
    let nul = 0u8;
    unsafe {
        copy_to_user(buf.add(cwd.len()), &nul as *const u8, 1)?;
    }
    // 返回 buf 指针
    Ok(buf as usize)
}

/// 系统调用 sys-pipe
pub fn sys_pipe(pipefd: *mut [usize; 2]) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let (pipe_read, pipe_write) = make_pipe();
    let mut fds = [0usize; 2];

    fds[0] = match task.alloc_fd(FdEntry::new(pipe_read, 0.into())) {
        Ok(fd) => fd,
        Err(e) => return Err(e),
    };
    fds[1] = match task.alloc_fd(FdEntry::new(pipe_write, 0.into())) {
        Ok(fd) => fd,
        Err(e) => {
            task.close(fds[0])?;
            return Err(e);
        },
    };

    if let Err(e) = copy_to_user(pipefd, &fds as *const [usize; 2], 1) {
        task.close(fds[0])?;
        task.close(fds[1])?;
        return Err(e);
    }

    Ok(0)
}
