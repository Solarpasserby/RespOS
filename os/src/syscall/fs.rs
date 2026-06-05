// os/src/syscall/fs.rs

use super::{Errno, SysResult};
use crate::fs::mount::{do_mount, do_umount2};
use crate::fs::vfs::{File, InodeType, OpenFlags};
use crate::fs::{
    AT_FDCWD, FdEntry, Stat, Statfs64, filename_create, filename_link, filename_lookup,
    filename_unlink, make_pipe, path_open,
};
use crate::mm::{check_user_writable, copy_cstr_from_user, copy_from_user, copy_to_user};
use crate::task::current_task;
use alloc::vec;

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

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IoVec {
    pub base: *mut u8,
    pub len: usize,
}

pub fn sys_writev(fd: usize, iov: *const IoVec, iovcnt: usize) -> SysResult<usize> {
    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return Err(Errno::EINVAL);
    }

    let mut total: usize = 0;
    for idx in 0..iovcnt {
        let mut item = IoVec {
            base: core::ptr::null_mut(),
            len: 0,
        };
        unsafe {
            copy_from_user(&mut item as *mut IoVec, iov.add(idx), 1)?;
        }
        if item.len == 0 {
            continue;
        }
        let written = sys_write(fd, item.base, item.len)?;
        total = total.checked_add(written).ok_or(Errno::EINVAL)?;
        if written < item.len {
            break;
        }
    }
    Ok(total)
}

/// 系统调用 sys-open
pub fn sys_openat(dirfd: isize, path: *const u8, flags: usize, mode: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let path = copy_cstr_from_user(path)?;
    let file = path_open(dirfd, path.as_str(), flags, mode)?;
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
    let resolved = filename_lookup(AT_FDCWD, path.as_str(), 0)?;
    let stat_buf: Stat = resolved
        .dentry
        .get_inode()
        .stat(&resolved.abs_path())?
        .into();
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

/// 系统调用 sys-fstatfs
pub fn sys_fstatfs(fd: usize, buf: *mut Statfs64) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    let file = file
        .as_any()
        .downcast_ref::<File>()
        .ok_or(Errno::EINVAL)?;
    let statfs = file.path().mnt.fs.statfs()?;
    copy_to_user(buf, &statfs as *const Statfs64, 1)?;
    Ok(0)
}

/// 系统调用 sys-lseek
pub fn sys_lseek(fd: usize, offset: isize, whence: usize) -> SysResult<usize> {
    const SEEK_SET: usize = 0;
    const SEEK_CUR: usize = 1;
    const SEEK_END: usize = 2;

    fn add_offset(base: usize, offset: isize) -> SysResult<usize> {
        if offset >= 0 {
            base.checked_add(offset as usize).ok_or(Errno::EINVAL)
        } else {
            let offset = offset.checked_neg().ok_or(Errno::EINVAL)? as usize;
            base.checked_sub(offset).ok_or(Errno::EINVAL)
        }
    }

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if file.get_stat()?.ty != InodeType::Regular {
        return Err(Errno::ESPIPE);
    }

    let new_offset = match whence {
        SEEK_SET => add_offset(0, offset)?,
        SEEK_CUR => add_offset(file.get_offset(), offset)?,
        SEEK_END => add_offset(file.get_stat()?.size, offset)?,
        _ => return Err(Errno::EINVAL),
    };
    file.seek(new_offset as isize)
}

/// 系统调用 sys-fcntl
pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> SysResult<usize> {
    const F_DUPFD: usize = 0;
    const F_GETFD: usize = 1;
    const F_SETFD: usize = 2;
    const F_GETFL: usize = 3;
    const F_SETFL: usize = 4;
    const F_DUPFD_CLOEXEC: usize = 1030;

    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd)?;

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            // close-on-exec 尚未实现, F_DUPFD_CLOEXEC 与 F_DUPFD 暂时等效
            task.alloc_fd_from(fd_entry, arg)
        }
        F_GETFD => Ok(0),
        F_SETFD => Ok(0),
        F_GETFL => Ok(fd_entry.get_flags().bits() as usize),
        F_SETFL => {
            let mut entry = fd_entry;
            entry.set_flags(OpenFlags::from(arg));
            task.set_fd(fd, entry)?;
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
}

/// 系统调用 sys-dup
pub fn sys_dup(fd: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd)?;
    task.alloc_fd(fd_entry)
}

/// 系统调用 sys-dup3
pub fn sys_dup3(fd_src: usize, fd_dst: usize, flags: usize) -> SysResult<usize> {
    // TODO[ABI-COMPAT]: 当前忽略 flags，尚未完整实现 dup3 语义。
    let _ = flags;
    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd_src)?;
    task.set_fd(fd_dst, fd_entry)?;
    Ok(fd_dst)
}

/// 系统调用 sys-mkdir
pub fn sys_mkdirat(dirfd: isize, path: *const u8, mode: usize) -> SysResult<usize> {
    let path = copy_cstr_from_user(path)?;
    filename_create(dirfd, path.as_str(), InodeType::Directory, mode)?;
    Ok(0)
}

/// 系统调用 sys-linkat
pub fn sys_linkat(
    olddirfd: isize,
    oldpath: *const u8,
    newdirfd: isize,
    newpath: *const u8,
    flags: usize,
) -> SysResult<usize> {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }

    let oldpath = copy_cstr_from_user(oldpath)?;
    let newpath = copy_cstr_from_user(newpath)?;
    filename_link(olddirfd, oldpath.as_str(), newdirfd, newpath.as_str())?;
    Ok(0)
}

/// 系统调用 sys-unlink
pub fn sys_unlinkat(dirfd: isize, path: *const u8, flags: usize) -> SysResult<usize> {
    const AT_REMOVEDIR: usize = 0x200;
    if flags & !AT_REMOVEDIR != 0 {
        return Err(Errno::EINVAL);
    }

    let path = copy_cstr_from_user(path)?;
    filename_unlink(dirfd, path.as_str(), flags & AT_REMOVEDIR != 0)?;
    Ok(0)
}

/// 系统调用 sys-chdir
pub fn sys_chdir(path: *const u8) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let path = copy_cstr_from_user(path)?;
    let resolved = filename_lookup(AT_FDCWD, path.as_str(), 0)?;
    task.set_cwd(resolved);
    Ok(0)
}

/// 系统调用 sys-getcwd
pub fn sys_getcwd(buf: *mut u8, len: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let cwd = task.cwd().global_abs_path();
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
pub fn sys_pipe2(pipefd: *mut [i32; 2], flags: usize) -> SysResult<usize> {
    // TODO[ABI-COMPAT]: 当前忽略 flags，尚未完整实现 pipe2 语义。
    let _ = flags;
    let task = current_task().expect("[kernel] current task is None.");
    let (pipe_read, pipe_write) = make_pipe();
    let mut fds = [0usize; 2];

    fds[0] = match task.alloc_fd(FdEntry::new(pipe_read, OpenFlags::O_RDONLY)) {
        Ok(fd) => fd,
        Err(e) => return Err(e),
    };
    fds[1] = match task.alloc_fd(FdEntry::new(pipe_write, OpenFlags::O_WRONLY)) {
        Ok(fd) => fd,
        Err(e) => {
            task.close(fds[0])?;
            return Err(e);
        }
    };

    let fds_ret = [fds[0] as i32, fds[1] as i32];
    if let Err(e) = copy_to_user(pipefd, &fds_ret as *const [i32; 2], 1) {
        task.close(fds[0])?;
        task.close(fds[1])?;
        return Err(e);
    }

    Ok(0)
}

/// 系统调用 sys-getdents64
pub fn sys_getdents64(fd: usize, dirp: *mut u8, count: usize) -> SysResult<usize> {
    if count == 0 {
        return Ok(0);
    }
    check_user_writable(dirp, count)?;

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.get_file();
    let file = if let Some(file_cast) = file.as_any().downcast_ref::<File>() {
        file_cast
    } else {
        return Err(Errno::ENOTDIR);
    };

    let mut offset = 0;
    let mut buf = vec![0u8; count];
    let dirents = file.readdir()?;
    for dirent in dirents {
        let dirent_size = dirent.d_reclen as usize;
        if dirent_size == 0 {
            return Err(Errno::EINVAL);
        }
        if offset + dirent_size > count {
            if offset == 0 {
                return Err(Errno::EINVAL);
            }
            break;
        }
        dirent.copy_to_buffer(&mut buf[offset..offset + dirent_size]);
        offset += dirent_size;
    }
    copy_to_user(dirp, buf.as_ptr(), offset)?;

    Ok(offset)
}

/// 系统调用 sys-mount
pub fn sys_mount(
    source: *const u8,
    target: *const u8,
    fstype: *const u8,
    flags: usize,
    _data: *const u8,
) -> SysResult<usize> {
    let _source_str = copy_cstr_from_user(source)?;
    let target_str = copy_cstr_from_user(target)?;
    let fstype_str = copy_cstr_from_user(fstype)?;
    do_mount(
        _source_str.as_str(),
        target_str.as_str(),
        fstype_str.as_str(),
        flags,
    )
}

/// 系统调用 sys-umount2
pub fn sys_umount2(target: *const u8, flags: usize) -> SysResult<usize> {
    let target = copy_cstr_from_user(target)?;
    do_umount2(target.as_str(), flags)
}

/// 系统调用 sys_readlinkat - 读取符号链接的目标路径
pub fn sys_readlinkat(
    dirfd: isize,
    path: *const u8,
    buf: *mut u8,
    bufsize: usize,
) -> SysResult<usize> {
    let path_str = copy_cstr_from_user(path)?;
    let target_path = filename_lookup(dirfd, &path_str, 0)?;
    let inode = target_path.dentry.get_inode();
    let link = inode.read_link(&target_path.abs_path())?;
    let bytes = link.as_bytes();
    let n = bytes.len().min(bufsize);
    if n > 0 {
        copy_to_user(buf, bytes.as_ptr(), n)?;
    }
    Ok(n)
}
