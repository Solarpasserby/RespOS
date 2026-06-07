// os/src/syscall/fs.rs

use super::{Errno, SysResult};
use crate::fs::mount::{do_mount, do_umount2};
use crate::fs::vfs::{File, FileOp, InodeType, OpenFlags};
use crate::fs::{
    AT_EMPTY_PATH, AT_FDCWD, AT_NO_AUTOMOUNT, AT_SYMLINK_NOFOLLOW, FdEntry, Stat, Statfs64,
    filename_create, filename_link, filename_lookup, filename_lookup_no_follow_final_symlink,
    filename_rename, filename_symlink, filename_unlink, make_pipe, path_open,
};
use crate::mm::{check_user_writable, copy_cstr_from_user, copy_from_user, copy_to_user};
use crate::task::current_task;
use crate::timer::{TimeSpec, get_time_ms};
use alloc::vec;

const UTIME_NOW: usize = 1_073_741_823;
const UTIME_OMIT: usize = 1_073_741_822;
const F_OK: usize = 0;
const X_OK: usize = 1;
const W_OK: usize = 2;
const R_OK: usize = 4;
const AT_EACCESS: usize = 0x200;

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

/// 系统调用 sys-fstatat
pub fn sys_fstatat(
    dirfd: isize,
    path: *const u8,
    stat: *mut Stat,
    flags: usize,
) -> SysResult<usize> {
    const FSTATAT_ALLOWED_FLAGS: usize = AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT | AT_EMPTY_PATH;

    if flags & !FSTATAT_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }

    let path = copy_cstr_from_user(path)?;
    // info!("Path: {}.", path);
    let stat_buf: Stat = if path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 {
            return Err(Errno::ENOENT);
        }
        if dirfd == AT_FDCWD {
            let task = current_task().expect("[kernel] current task is None.");
            let cwd = task.cwd();
            cwd.dentry.get_inode().stat(&cwd.abs_path())?
        } else {
            if dirfd < 0 {
                return Err(Errno::EBADF);
            }
            let task = current_task().expect("[kernel] current task is None.");
            task.get_fd_entry(dirfd as usize)?.file.get_stat()?
        }
    } else {
        // 默认 stat 跟随最终 symlink；带 AT_SYMLINK_NOFOLLOW 时退化为 lstat 语义。
        let resolved = if flags & AT_SYMLINK_NOFOLLOW != 0 {
            filename_lookup_no_follow_final_symlink(dirfd, path.as_str())?
        } else {
            filename_lookup(dirfd, path.as_str(), 0)?
        };
        resolved.dentry.get_inode().stat(&resolved.abs_path())?
    }
    .into();
    copy_to_user(stat, &stat_buf as *const Stat, 1)?;
    Ok(0)
}

/// 检查 utimensat 传入的 timespec 是否合法。
///
/// Linux 约定 nsec 可以是正常纳秒值，也可以是两个特殊值：
/// UTIME_NOW 表示使用当前时间，UTIME_OMIT 表示保持原值不变。
fn validate_utimens_time(ts: TimeSpec) -> SysResult<TimeSpec> {
    if ts.nsec == UTIME_NOW || ts.nsec == UTIME_OMIT || ts.nsec < 1_000_000_000 {
        Ok(ts)
    } else {
        Err(Errno::EINVAL)
    }
}

fn current_timespec() -> TimeSpec {
    let ms = get_time_ms();
    TimeSpec {
        sec: ms / 1000,
        nsec: (ms % 1000) * 1_000_000,
    }
}

/// 将用户传入的 times[2] 解析为需要写入 inode 的 atime/mtime。
///
/// 返回值中的 None 表示该时间戳保持不变；Some 表示需要写入。
/// times 为 NULL 时，atime 和 mtime 都设置为当前时间。
fn resolve_utimens_times(
    times: *const TimeSpec,
) -> SysResult<(Option<TimeSpec>, Option<TimeSpec>)> {
    if times.is_null() {
        let now = current_timespec();
        return Ok((Some(now), Some(now)));
    }

    let mut ts = [TimeSpec::default(); 2];
    copy_from_user(ts.as_mut_ptr(), times, 2)?;
    let atime = validate_utimens_time(ts[0])?;
    let mtime = validate_utimens_time(ts[1])?;
    let now = current_timespec();

    let atime = match atime.nsec {
        UTIME_OMIT => None,
        UTIME_NOW => Some(now),
        _ => Some(atime),
    };
    let mtime = match mtime.nsec {
        UTIME_OMIT => None,
        UTIME_NOW => Some(now),
        _ => Some(mtime),
    };
    Ok((atime, mtime))
}

/// 系统调用 sys-utimensat
///
/// 修改文件的访问时间 atime 和修改时间 mtime，常见调用者是 touch。
/// 当前 ext4 后端只保存秒级时间，因此纳秒字段只参与合法性和特殊值判断。
pub fn sys_utimensat(
    dirfd: isize,
    path: *const u8,
    times: *const TimeSpec,
    flags: usize,
) -> SysResult<usize> {
    const UTIMENSAT_ALLOWED_FLAGS: usize = AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH;

    if flags & !UTIMENSAT_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }

    let (atime, mtime) = resolve_utimens_times(times)?;
    if atime.is_none() && mtime.is_none() {
        return Ok(0);
    }

    let path = copy_cstr_from_user(path)?;
    // 空路径只有在 AT_EMPTY_PATH 下合法：此时修改 dirfd 指向的文件；
    // 若 dirfd 是 AT_FDCWD，则修改当前工作目录。
    if path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 {
            return Err(Errno::ENOENT);
        }
        if dirfd == AT_FDCWD {
            let task = current_task().expect("[kernel] current task is None.");
            let cwd = task.cwd();
            cwd.dentry
                .get_inode()
                .set_times(&cwd.abs_path(), atime, mtime)?;
        } else {
            if dirfd < 0 {
                return Err(Errno::EBADF);
            }
            let task = current_task().expect("[kernel] current task is None.");
            let file = task.get_fd_entry(dirfd as usize)?.file;
            let file = file.as_any().downcast_ref::<File>().ok_or(Errno::EINVAL)?;
            let path = file.path();
            file.inode().set_times(&path.abs_path(), atime, mtime)?;
        }
    } else {
        // utimensat 同样根据 AT_SYMLINK_NOFOLLOW 决定修改链接本身还是链接目标。
        let resolved = if flags & AT_SYMLINK_NOFOLLOW != 0 {
            filename_lookup_no_follow_final_symlink(dirfd, path.as_str())?
        } else {
            filename_lookup(dirfd, path.as_str(), 0)?
        };
        resolved
            .dentry
            .get_inode()
            .set_times(&resolved.abs_path(), atime, mtime)?;
    }

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

/// 系统调用 sys-faccessat
///
/// 按 dirfd + path 定位文件，并根据 mode 检查该路径是否存在、是否可读/可写/可执行。
/// 它只回答“权限检查是否通过”，不会打开文件，也不会返回文件状态结构。
/// 用户态的 access()/faccessat() 常用它在真正 exec/open 前做一次轻量探测，
/// 例如 busybox which 会用 X_OK 判断 PATH 中的命令文件是否可执行。
///
/// mode 可以是 F_OK，或 R_OK/W_OK/X_OK 的组合：F_OK 只要求路径存在，
/// 其它位会继续检查 inode mode 中至少有一类用户具备对应权限。
/// dirfd 与相对路径的解释交给 namei；绝对路径会自然忽略 dirfd。
/// flags 目前只接受 AT_EACCESS、AT_SYMLINK_NOFOLLOW 和 AT_EMPTY_PATH，
/// 其中 AT_SYMLINK_NOFOLLOW 会让最后一级符号链接停在链接本身。
///
/// 当前内核还没有完整的 uid/gid 权限模型，这里先检查路径是否存在，
/// 并用 inode mode 的基础权限位满足 busybox/coreutils 的可执行性探测。
///
/// TODO[ABI-COMPAT]: Linux access/faccessat 默认使用 real uid/gid，
/// faccessat2 + AT_EACCESS 才使用 effective uid/gid；当前暂未区分。
/// TODO[ABI-COMPAT]: 尚未实现 root/capability/ACL 等权限放宽规则。
/// TODO[ABI-COMPAT]: 尚未检查路径中每一级目录的 search/execute 权限。
/// TODO[ABI-COMPAT]: W_OK 对只读挂载、不可变文件等特殊状态的语义尚未实现。
pub fn sys_faccessat(
    dirfd: isize,
    path: *const u8,
    mode: usize,
    flags: usize,
) -> SysResult<usize> {
    const FACCESSAT_ALLOWED_FLAGS: usize = AT_EACCESS | AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH;

    if mode & !(R_OK | W_OK | X_OK) != 0 || flags & !FACCESSAT_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }

    let path = copy_cstr_from_user(path)?;
    let kstat = if path.is_empty() {
        // TODO[ABI-COMPAT]: AT_EMPTY_PATH 是 faccessat2 扩展；
        // 若未来同时暴露旧 faccessat，需要按 syscall 入口区分 flags 语义。
        if flags & AT_EMPTY_PATH == 0 {
            return Err(Errno::ENOENT);
        }
        if dirfd == AT_FDCWD {
            let task = current_task().expect("[kernel] current task is None.");
            let cwd = task.cwd();
            cwd.dentry.get_inode().stat(&cwd.abs_path())?
        } else {
            if dirfd < 0 {
                return Err(Errno::EBADF);
            }
            let task = current_task().expect("[kernel] current task is None.");
            task.get_fd_entry(dirfd as usize)?.file.get_stat()?
        }
    } else {
        // TODO[ABI-COMPAT]: AT_SYMLINK_NOFOLLOW 下 Linux 检查链接本身；
        // 当前符号链接默认权限按 0777 处理，尚未覆盖特殊 LSM/ACL 行为。
        let resolved = if flags & AT_SYMLINK_NOFOLLOW != 0 {
            filename_lookup_no_follow_final_symlink(dirfd, path.as_str())?
        } else {
            filename_lookup(dirfd, path.as_str(), 0)?
        };
        resolved.dentry.get_inode().stat(&resolved.abs_path())?
    };

    if mode == F_OK {
        return Ok(0);
    }

    let perm = access_perm_bits(kstat.ty, kstat.mode);
    if mode & R_OK != 0 && perm & 0o444 == 0 {
        return Err(Errno::EACCES);
    }
    if mode & W_OK != 0 && perm & 0o222 == 0 {
        return Err(Errno::EACCES);
    }
    if mode & X_OK != 0 && perm & 0o111 == 0 {
        return Err(Errno::EACCES);
    }
    Ok(0)
}

fn access_perm_bits(ty: InodeType, mode: u32) -> u32 {
    let perm = mode & 0o777;
    if perm != 0 {
        return perm;
    }
    // TODO[ABI-COMPAT]: 虚拟文件系统暂缺稳定 mode 时使用默认权限兜底；
    // 后续应由各 inode 后端返回更精确的权限位。
    match ty {
        InodeType::Directory => 0o755,
        InodeType::Regular => 0o644,
        InodeType::SymLink => 0o777,
        _ => 0o666,
    }
}

/// 系统调用 sys-statfs
pub fn sys_statfs(path: *const u8, buf: *mut Statfs64) -> SysResult<usize> {
    let path = copy_cstr_from_user(path)?;
    let resolved = filename_lookup(AT_FDCWD, path.as_str(), 0)?;
    let statfs = resolved.mnt.fs.statfs()?;
    copy_to_user(buf, &statfs as *const Statfs64, 1)?;
    Ok(0)
}

/// 系统调用 sys-fstatfs
pub fn sys_fstatfs(fd: usize, buf: *mut Statfs64) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    let file = file.as_any().downcast_ref::<File>().ok_or(Errno::EINVAL)?;
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
    let ty = file.get_stat()?.ty;
    if ty != InodeType::Regular && ty != InodeType::Directory {
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

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct WinSize {
    row: u16,
    col: u16,
    xpixel: u16,
    ypixel: u16,
}

/// 系统调用 sys-ioctl
///
/// ioctl 是设备相关的杂项控制入口，真实 Linux 会按文件对应的驱动分发。
/// 这里先补 BusyBox/musl 常见的终端窗口大小探测：stdio 在第一次输出时
/// 可能用 TIOCGWINSZ 判断终端宽度和行缓冲策略，od 的格式化输出也会经过这条路径。
///
/// TODO[ABI-COMPAT]: 终端 TCGETS/TCSETS、RTC、块设备、网络设备等 ioctl
/// 需要下沉到具体 FileOp/设备驱动中实现，不能长期放在 syscall 层硬编码。
pub fn sys_ioctl(fd: usize, request: usize, arg: usize) -> SysResult<usize> {
    const TIOCGWINSZ: usize = 0x5413;

    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd)?;

    match request {
        TIOCGWINSZ if fd_entry.file.is_tty() => {
            let winsize = WinSize {
                row: 24,
                col: 80,
                xpixel: 0,
                ypixel: 0,
            };
            copy_to_user(arg as *mut WinSize, &winsize as *const WinSize, 1)?;
            Ok(0)
        }
        _ => Err(Errno::ENOTTY),
    }
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

/// 系统调用 sys-symlinkat
pub fn sys_symlinkat(target: *const u8, newdirfd: isize, linkpath: *const u8) -> SysResult<usize> {
    let target = copy_cstr_from_user(target)?;
    let linkpath = copy_cstr_from_user(linkpath)?;
    filename_symlink(newdirfd, target.as_str(), linkpath.as_str())?;
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

/// 系统调用 sys-renameat2
pub fn sys_renameat2(
    olddirfd: isize,
    oldpath: *const u8,
    newdirfd: isize,
    newpath: *const u8,
    flags: usize,
) -> SysResult<usize> {
    const RENAME_NOREPLACE: usize = 1;
    const RENAME_EXCHANGE: usize = 2;
    const RENAME_ALLOWED_FLAGS: usize = RENAME_NOREPLACE | RENAME_EXCHANGE;

    if flags & !RENAME_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    if flags & RENAME_EXCHANGE != 0 {
        return Err(Errno::ENOSYS);
    }

    let oldpath = copy_cstr_from_user(oldpath)?;
    let newpath = copy_cstr_from_user(newpath)?;
    filename_rename(olddirfd, oldpath.as_str(), newdirfd, newpath.as_str())?;
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

    let current_off = file.get_offset();
    let mut written = 0;
    let mut next_off = current_off;
    let mut buf = vec![0u8; count];
    let dirents = file.readdir()?;
    for dirent in dirents {
        let dirent_off = usize::try_from(dirent.d_off).map_err(|_| Errno::EINVAL)?;
        if dirent_off <= current_off {
            continue;
        }

        let dirent_size = dirent.d_reclen as usize;
        if dirent_size == 0 {
            return Err(Errno::EINVAL);
        }
        if written + dirent_size > count {
            if written == 0 {
                return Err(Errno::EINVAL);
            }
            break;
        }
        dirent.copy_to_buffer(&mut buf[written..written + dirent_size]);
        written += dirent_size;
        next_off = dirent_off;
    }

    if written != 0 {
        let next_off = isize::try_from(next_off).map_err(|_| Errno::EINVAL)?;
        file.seek(next_off)?;
        copy_to_user(dirp, buf.as_ptr(), written)?;
    }

    Ok(written)
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
    // readlinkat 读取的是最后一级 symlink inode 的 payload，不能先跟随到目标文件。
    let target_path = filename_lookup_no_follow_final_symlink(dirfd, &path_str)?;
    let inode = target_path.dentry.get_inode();
    let link = inode.read_link(&target_path.abs_path())?;
    let bytes = link.as_bytes();
    let n = bytes.len().min(bufsize);
    if n > 0 {
        copy_to_user(buf, bytes.as_ptr(), n)?;
    }
    Ok(n)
}
