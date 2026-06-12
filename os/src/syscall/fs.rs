// os/src/syscall/fs.rs

use super::{Errno, SysResult};
use crate::config::PAGE_SIZE;
use crate::fs::mount::{do_mount, do_umount2};
use crate::fs::vfs::InodeType;
use crate::fs::{
    AT_EMPTY_PATH, AT_FDCWD, AT_NO_AUTOMOUNT, AT_SYMLINK_NOFOLLOW, FdEntry, File, FileOp, KStat,
    OpenFlags, Stat, Statfs64, filename_create, filename_link, filename_lookup,
    filename_lookup_no_follow_final_symlink, filename_rename, filename_symlink, filename_unlink,
    init_fdset, make_pipe, path_open,
};
use crate::mm::{
    VPNRange, VirtAddr, check_user_writable, copy_cstr_from_user, copy_from_user, copy_to_user,
};
use crate::signal::sig_struct::{Sig, SigSet};
use crate::task::{current_task, yield_current_task};
use crate::timer::{TimeSpec, get_time_ms, get_timeout_us};

const UTIME_NOW: isize = 1_073_741_823;
const UTIME_OMIT: isize = 1_073_741_822;
const F_OK: usize = 0;
const X_OK: usize = 1;
const W_OK: usize = 2;
const R_OK: usize = 4;
const AT_EACCESS: usize = 0x200;
const AT_STATX_SYNC_TYPE: usize = 0x6000;

// 使用 mm 实现的 `copy_cstr_from_user`, `copy_from_user`, `copy_to_user` 来访问用户空间的数据

// TODO: write 和 read 借助堆上分配的空间中转数据，有额外开销，须优化
const IO_CHUNK_SIZE: usize = PAGE_SIZE;

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

    let mut kbuf = alloc::vec![0u8; len.min(IO_CHUNK_SIZE)];
    let mut total = 0usize;
    while total < len {
        let chunk_len = (len - total).min(kbuf.len());
        let ret = file.read(&mut kbuf[..chunk_len])?;
        if ret == 0 {
            break;
        }
        copy_to_user(unsafe { buf.add(total) }, kbuf.as_ptr(), ret)?;
        total += ret;
        if ret < chunk_len {
            break;
        }
    }
    Ok(total)
}

pub fn sys_pread64(fd: usize, buf: *mut u8, len: usize, offset: isize) -> SysResult<usize> {
    if offset < 0 {
        return Err(Errno::EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    check_user_writable(buf, len)?;

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.readable() {
        return Err(Errno::EBADF);
    }
    file.can_seek()?;
    let old_offset = file.get_offset();
    file.seek(offset)?;
    let mut kbuf = alloc::vec![0u8; len.min(IO_CHUNK_SIZE)];
    let mut total = 0usize;
    let mut ret = Ok(0usize);
    while total < len {
        let chunk_len = (len - total).min(kbuf.len());
        ret = file.read(&mut kbuf[..chunk_len]);
        let read_len = match ret {
            Ok(read_len) => read_len,
            Err(_) => break,
        };
        if read_len == 0 {
            break;
        }
        copy_to_user(unsafe { buf.add(total) }, kbuf.as_ptr(), read_len)?;
        total += read_len;
        if read_len < chunk_len {
            break;
        }
    }
    let restore_ret = file.seek(old_offset as isize);

    match (ret, restore_ret) {
        (Ok(_), Ok(_)) => Ok(total),
        (Err(err), _) => Err(err),
        (_, Err(err)) => Err(err),
    }
}

pub fn sys_pwrite64(fd: usize, buf: *mut u8, len: usize, offset: isize) -> SysResult<usize> {
    if offset < 0 {
        return Err(Errno::EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.writable() {
        return Err(Errno::EBADF);
    }
    file.can_seek()?;

    let old_offset = file.get_offset();
    file.seek(offset)?;

    let mut kbuf = alloc::vec![0u8; len.min(IO_CHUNK_SIZE)];
    let mut total = 0usize;
    let mut ret = Ok(0usize);
    while total < len {
        let chunk_len = (len - total).min(kbuf.len());
        if let Err(err) = copy_from_user(kbuf.as_mut_ptr(), unsafe { buf.add(total) }, chunk_len) {
            ret = Err(err);
            break;
        }
        ret = file.write(&kbuf[..chunk_len]);
        let written = match ret {
            Ok(written) => written,
            Err(_) => break,
        };
        total += written;
        if written < chunk_len {
            break;
        }
    }

    let restore_ret = file.seek(old_offset as isize);
    match (ret, restore_ret) {
        (Ok(_), Ok(_)) => Ok(total),
        (Err(err), _) if total == 0 => Err(err),
        (Err(_), _) => Ok(total),
        (_, Err(err)) if total == 0 => Err(err),
        (_, Err(_)) => Ok(total),
    }
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

    let mut kbuf = alloc::vec![0u8; len.min(IO_CHUNK_SIZE)];
    let mut total = 0usize;
    while total < len {
        let chunk_len = (len - total).min(kbuf.len());
        copy_from_user(kbuf.as_mut_ptr(), unsafe { buf.add(total) }, chunk_len)?;
        let written = file.write(&kbuf[..chunk_len])?;
        total += written;
        if written < chunk_len {
            break;
        }
    }
    Ok(total)
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
        let iov_ret = unsafe { copy_from_user(&mut item as *mut IoVec, iov.add(idx), 1) };
        if let Err(err) = iov_ret {
            return if total > 0 { Ok(total) } else { Err(err) };
        }
        if item.len == 0 {
            continue;
        }
        let written = match sys_write(fd, item.base, item.len) {
            Ok(written) => written,
            Err(err) => return if total > 0 { Ok(total) } else { Err(err) },
        };
        total = total.checked_add(written).ok_or(Errno::EINVAL)?;
        if written < item.len {
            break;
        }
    }
    Ok(total)
}

pub fn sys_readv(fd: usize, iov: *const IoVec, iovcnt: usize) -> SysResult<usize> {
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
        let iov_ret = unsafe { copy_from_user(&mut item as *mut IoVec, iov.add(idx), 1) };
        if let Err(err) = iov_ret {
            return if total > 0 { Ok(total) } else { Err(err) };
        }
        if item.len == 0 {
            continue;
        }
        let read = match sys_read(fd, item.base, item.len) {
            Ok(read) => read,
            Err(err) => return if total > 0 { Ok(total) } else { Err(err) },
        };
        total = total.checked_add(read).ok_or(Errno::EINVAL)?;
        if read < item.len {
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
    let stat_buf: Stat = stat_at(dirfd, path.as_str(), flags)?.into();
    copy_to_user(stat, &stat_buf as *const Stat, 1)?;
    Ok(0)
}

fn stat_at(dirfd: isize, path: &str, flags: usize) -> SysResult<KStat> {
    let kstat = if path.is_empty() {
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
            filename_lookup_no_follow_final_symlink(dirfd, path)?
        } else {
            filename_lookup(dirfd, path, 0)?
        };
        resolved.dentry.get_inode().stat(&resolved.abs_path())?
    };
    Ok(kstat)
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct StatxTimestamp {
    sec: i64,
    nsec: u32,
    _pad: i32,
}

impl From<TimeSpec> for StatxTimestamp {
    fn from(ts: TimeSpec) -> Self {
        Self {
            sec: ts.sec as i64,
            nsec: ts.nsec as u32,
            _pad: 0,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Statx {
    stx_mask: u32,
    stx_blksize: u32,
    stx_attributes: u64,
    stx_nlink: u32,
    stx_uid: u32,
    stx_gid: u32,
    stx_mode: u16,
    _spare0: [u16; 1],
    stx_ino: u64,
    stx_size: u64,
    stx_blocks: u64,
    stx_attributes_mask: u64,
    stx_atime: StatxTimestamp,
    stx_btime: StatxTimestamp,
    stx_ctime: StatxTimestamp,
    stx_mtime: StatxTimestamp,
    stx_rdev_major: u32,
    stx_rdev_minor: u32,
    stx_dev_major: u32,
    stx_dev_minor: u32,
    stx_mnt_id: u64,
    stx_dio_mem_align: u32,
    stx_dio_offset_align: u32,
    _spare3: [u64; 12],
}

impl From<KStat> for Statx {
    fn from(kstat: KStat) -> Self {
        const STATX_BASIC_STATS: u32 = 0x0000_07ff;
        let stat: Stat = kstat.into();
        Self {
            stx_mask: STATX_BASIC_STATS,
            stx_blksize: stat.st_blksize,
            stx_nlink: stat.st_nlink,
            stx_uid: stat.st_uid,
            stx_gid: stat.st_gid,
            stx_mode: stat.st_mode as u16,
            stx_ino: stat.st_ino,
            stx_size: stat.st_size,
            stx_blocks: stat.st_blocks,
            stx_atime: stat.st_atime.into(),
            stx_ctime: stat.st_ctime.into(),
            stx_mtime: stat.st_mtime.into(),
            ..Default::default()
        }
    }
}

/// 系统调用 sys-statx
///
/// LoongArch 的 musl/busybox 会优先用 statx 实现 stat/lstat/access 前的
/// metadata 查询。这里先提供 basic stats，使它与现有 fstatat 共享路径解析。
pub fn sys_statx(
    dirfd: isize,
    path: *const u8,
    flags: usize,
    mask: u32,
    buf: *mut Statx,
) -> SysResult<usize> {
    const STATX_RESERVED: u32 = 0x8000_0000;
    const STATX_ALLOWED_FLAGS: usize =
        AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT | AT_EMPTY_PATH | AT_STATX_SYNC_TYPE;

    if mask & STATX_RESERVED != 0 || flags & !STATX_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }

    let path = copy_cstr_from_user(path)?;
    let statx: Statx = stat_at(dirfd, path.as_str(), flags)?.into();
    copy_to_user(buf, &statx as *const Statx, 1)?;
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
        sec: (ms / 1000) as isize,
        nsec: ((ms % 1000) * 1_000_000) as isize,
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

fn set_fd_times(fd: isize, atime: Option<TimeSpec>, mtime: Option<TimeSpec>) -> SysResult {
    if fd < 0 {
        return Err(Errno::EBADF);
    }
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd as usize)?.file;
    let file = file.as_any().downcast_ref::<File>().ok_or(Errno::EINVAL)?;
    let path = file.path();
    file.inode().set_times(&path.abs_path(), atime, mtime)
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

    // futimens(fd, times) 在 libc 中常落成 utimensat(fd, NULL, times, 0)。
    // NULL path 与空字符串不同：它直接表示 dirfd 指向的打开文件。
    if path.is_null() {
        return set_fd_times(dirfd, atime, mtime).map(|_| 0);
    }

    let path = copy_cstr_from_user(path)?;
    // 空路径只有在 AT_EMPTY_PATH 下合法：此时修改 dirfd 指向的文件；
    // 若 dirfd 是 AT_FDCWD，则修改当前工作目录。
    if path.is_empty() {
        if dirfd != AT_FDCWD {
            // musl 的 futimens(fd, ...) 可能落成 utimensat(fd, "", times, 0)。
            // 这里把非 AT_FDCWD 的空路径按 fd 目标处理，避免已 unlink 的打开文件
            // 只能通过路径更新而失败。
            set_fd_times(dirfd, atime, mtime)?;
        } else {
            if flags & AT_EMPTY_PATH == 0 {
                return Err(Errno::ENOENT);
            }
            let task = current_task().expect("[kernel] current task is None.");
            let cwd = task.cwd();
            cwd.dentry
                .get_inode()
                .set_times(&cwd.abs_path(), atime, mtime)?;
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

pub fn sys_ftruncate(fd: usize, length: isize) -> SysResult<usize> {
    if length < 0 {
        return Err(Errno::EINVAL);
    }
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.writable() {
        return Err(Errno::EINVAL);
    }
    let file = file.as_any().downcast_ref::<File>().ok_or(Errno::EINVAL)?;
    file.truncate(length as usize)
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
pub fn sys_faccessat(dirfd: isize, path: *const u8, mode: usize, flags: usize) -> SysResult<usize> {
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
    file.can_seek()?;

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
    const FD_CLOEXEC: usize = 1;

    let task = current_task().expect("[kernel] current task is None.");
    let fd_entry = task.get_fd_entry(fd)?;

    match cmd {
        F_DUPFD => task.alloc_fd_from(fd_entry, arg),
        F_DUPFD_CLOEXEC => {
            let mut entry = fd_entry;
            entry.set_flags(entry.get_flags() | OpenFlags::O_CLOEXEC);
            task.alloc_fd_from(entry, arg)
        }
        F_GETFD => {
            if fd_entry.get_flags().contains(OpenFlags::O_CLOEXEC) {
                Ok(FD_CLOEXEC)
            } else {
                Ok(0)
            }
        }
        F_SETFD => {
            let mut entry = fd_entry;
            let mut flags = entry.get_flags();
            if arg & FD_CLOEXEC != 0 {
                flags |= OpenFlags::O_CLOEXEC;
            } else {
                flags.remove(OpenFlags::O_CLOEXEC);
            }
            entry.set_flags(flags);
            task.set_fd(fd, entry)?;
            Ok(0)
        }
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
        let mut record = alloc::vec![0u8; dirent_size];
        dirent.copy_to_buffer(&mut record);
        let dst = unsafe { dirp.add(written) };
        copy_to_user(dst, record.as_ptr(), dirent_size)?;
        written += dirent_size;
        next_off = dirent_off;
    }

    if written != 0 {
        let next_off = isize::try_from(next_off).map_err(|_| Errno::EINVAL)?;
        file.seek(next_off)?;
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

/// pselect6 的 sigmask 参数不是单纯的 sigset_t*，而是一个（sigset_t* + size_t）对。
/// musl/glibc 都会把这个结构体的地址传给内核。
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Pselect6Sigmask {
    sigmask: usize,
    sigsetsize: usize,
}

/// 解析 pselect6 超时参数。
///
/// - `None`   → 无限等待，直到有 fd 就绪或信号中断
/// - `Some(0)` → 不等待，立即返回当前就绪状态
/// - `Some(t)` → 等待最多 t 微秒
fn pselect_timeout_us(timeout: *const TimeSpec) -> SysResult<Option<usize>> {
    if timeout.is_null() {
        return Ok(None);
    }

    let mut tmo = TimeSpec::default();
    copy_from_user(&mut tmo as *mut TimeSpec, timeout, 1)?;
    tmo.checked_duration_us().ok_or(Errno::EINVAL).map(Some)
}

/// 解析 pselect6 信号掩码参数。
///
/// `sigmask == 0` 表示不修改掩码（类似 select 行为）。
/// 否则从用户空间读取 sigset_t，过滤掉不可屏蔽的 SIGKILL/SIGSTOP。
fn pselect_sigmask(sigmask: usize) -> SysResult<Option<SigSet>> {
    if sigmask == 0 {
        return Ok(None);
    }

    let mut user_arg = Pselect6Sigmask::default();
    copy_from_user(
        &mut user_arg as *mut Pselect6Sigmask,
        sigmask as *const Pselect6Sigmask,
        1,
    )?;
    if user_arg.sigmask == 0 {
        return Ok(None);
    }
    if user_arg.sigsetsize != core::mem::size_of::<SigSet>() {
        return Err(Errno::EINVAL);
    }

    let mut new_mask = SigSet::empty();
    copy_from_user(
        &mut new_mask as *mut SigSet,
        user_arg.sigmask as *const SigSet,
        1,
    )?;
    new_mask.remove_signal(Sig::SIGKILL);
    new_mask.remove_signal(Sig::SIGSTOP);
    Ok(Some(new_mask))
}

/// pselect6 — 等待多个文件描述符就绪，带超时和信号掩码。
///
/// 退出条件（任一满足即返回）：
/// 1. 有 fd 可读/可写 → 返回就绪 fd 数
/// 2. 超时到期 → 返回 0
/// 3. 被信号中断 → 返回 EINTR
///
/// sigmask 允许原子替换信号掩码；函数返回后自动恢复原掩码。
pub fn sys_pselect6(
    nfds: usize,
    readfds: usize,
    writefds: usize,
    exceptfds: usize,
    timeout: *const TimeSpec,
    sigmask: usize,
) -> SysResult<usize> {
    let timeout_us = pselect_timeout_us(timeout)?;
    let new_mask = pselect_sigmask(sigmask)?;

    // 保存并替换信号掩码（pselect6 的 sigmask 参数语义）
    let task = current_task().expect("[kernel] current task is None.");
    let origin_mask = task.op_sig_pending(|pending| pending.mask);
    if let Some(mask) = new_mask {
        task.op_sig_pending_mut(|pending| pending.mask = mask);
    }

    // 闭包保证 cleanup（恢复掩码）在任意退出路径上都执行
    let result = (|| {
        let start_us = get_timeout_us();
        let mut readfditer = init_fdset(readfds, nfds)?;
        let mut writeiter = init_fdset(writefds, nfds)?;
        let mut exceptiter = init_fdset(exceptfds, nfds)?;

        let mut set;
        loop {
            set = 0;

            // 轮询可读 fd：fd 必须是以读模式打开 且 数据立即可用
            if readfditer.fdset.valid() {
                readfditer.fdset.clear();
                for i in 0..readfditer.fds.len() {
                    let fd = readfditer.fds[i];
                    let file = &readfditer.files[i];
                    if file.readable() && file.read_ready() {
                        readfditer.fdset.set(fd);
                        set += 1;
                    }
                }
            }

            // 轮询可写 fd：fd 必须是以写模式打开 且 缓冲区有空间
            if writeiter.fdset.valid() {
                writeiter.fdset.clear();
                for i in 0..writeiter.fds.len() {
                    let fd = writeiter.fds[i];
                    let file = &writeiter.files[i];
                    if file.writable() && file.write_ready() {
                        writeiter.fdset.set(fd);
                        set += 1;
                    }
                }
            }

            // 当前文件对象没有 out-of-band/异常事件来源，exceptfds 只做合法性检查和清零写回。
            if exceptiter.fdset.valid() {
                exceptiter.fdset.clear();
            }

            if set > 0 {
                break;
            }

            if let Some(timeout_us) = timeout_us {
                if timeout_us == 0 {
                    break;
                }
                let elapsed_us = get_timeout_us().saturating_sub(start_us);
                if elapsed_us >= timeout_us {
                    break;
                }
            }

            // 在 yield 前标记可中断，让信号能在调度器中被检测到
            task.set_interruptible(true);
            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }
            yield_current_task();
            // yield 后再次检查：其他任务或信号可能设置了中断标志
            if task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }
            task.set_interruptible(false);
        }

        // 将内核修改后的 fd_set 写回用户空间
        if readfditer.fdset.valid() {
            readfditer.fdset.write_back()?;
        }
        if writeiter.fdset.valid() {
            writeiter.fdset.write_back()?;
        }
        if exceptiter.fdset.valid() {
            exceptiter.fdset.write_back()?;
        }

        Ok(set)
    })();

    // 确保无论如何退出都恢复原状态
    task.set_interruptible(false);
    if new_mask.is_some() {
        task.op_sig_pending_mut(|pending| pending.mask = origin_mask);
    }

    result
}

/// 系统调用 sys-fsync — 将文件缓冲数据刷入存储介质。
/// 当前文件系统实现在内存中，直接返回成功。
pub fn sys_fsync(fd: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    file.fsync()
}

/// 系统调用 sys-fdatasync — 当前等价于 fsync。
pub fn sys_fdatasync(fd: usize) -> SysResult<usize> {
    sys_fsync(fd)
}

/// 系统调用 sys-msync — 同步 mmap 映射区域与文件。
///
/// 支持的 flags：
/// - MS_ASYNC (1)：异步写回。当前无操作。
/// - MS_INVALIDATE (2)：当前无页缓存失效实现，仅做参数与地址校验。
/// - MS_SYNC (4)：同步写回。当前无操作。
///
/// MS_ASYNC 和 MS_SYNC 互斥。
pub fn sys_msync(addr: usize, len: usize, flags: i32) -> SysResult<usize> {
    const MS_ASYNC: i32 = 1;
    const MS_INVALIDATE: i32 = 2;
    const MS_SYNC: i32 = 4;
    const MS_ALLOWED_FLAGS: i32 = MS_ASYNC | MS_INVALIDATE | MS_SYNC;

    if addr % PAGE_SIZE != 0 {
        return Err(Errno::EINVAL);
    }
    if flags & !MS_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    if flags & MS_ASYNC != 0 && flags & MS_SYNC != 0 {
        return Err(Errno::EINVAL);
    }

    if len == 0 {
        return Ok(0);
    }

    let end = addr.checked_add(len).ok_or(Errno::EINVAL)?;
    let start_vpn = VirtAddr::from(addr).floor();
    let end_vpn = VirtAddr::from(end).ceil();
    let vpn_range = VPNRange::new(start_vpn, end_vpn);
    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_read(|memory_set| memory_set.check_user_mapped_range(vpn_range))?;

    Ok(0)
}

/// 系统调用 preadv — 从指定文件偏移处读取数据，分散写入多个用户缓冲区。
///
///
/// 语义细节：
/// - 中途出错且已有部分数据读取时，返回已读字节数而非 -1
/// - 短读（read 返回不足请求长度）直接终止，不再处理后续 iov
pub fn sys_preadv(
    fd: usize,
    iov_ptr: *const IoVec,
    iovcnt: usize,
    offset: isize,
) -> SysResult<usize> {
    const IOV_MAX: usize = 1024;
    if offset < 0 || iovcnt > IOV_MAX {
        return Err(Errno::EINVAL);
    }
    if iovcnt == 0 {
        return Ok(0);
    }

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.readable() {
        return Err(Errno::EBADF);
    }
    file.can_seek()?;

    let old_offset = file.get_offset(); // 保存原偏移
    file.seek(offset)?; // 定位到写入起点

    let mut total: usize = 0;
    for idx in 0..iovcnt {
        let mut item = IoVec {
            base: core::ptr::null_mut(),
            len: 0,
        };
        let iov_ret = unsafe { copy_from_user(&mut item as *mut IoVec, iov_ptr.add(idx), 1) };
        if let Err(err) = iov_ret {
            let _ = file.seek(old_offset as isize);
            return if total > 0 { Ok(total) } else { Err(err) };
        }
        if item.len == 0 {
            continue;
        }
        // 复用 sys_read 完成单段读取（内部走 file.read()，偏移自动推进）
        match sys_read(fd, item.base, item.len) {
            Ok(read) => {
                total = total.checked_add(read).ok_or(Errno::EINVAL)?;
                if read < item.len {
                    break; // 短读：文件已读完，不再续读后续 iov
                }
            }
            Err(err) => {
                let _ = file.seek(old_offset as isize);
                return if total > 0 { Ok(total) } else { Err(err) };
            }
        }
    }

    let _ = file.seek(old_offset as isize); // 恢复原偏移
    Ok(total)
}

/// 系统调用 pwritev — 将多个用户缓冲区的数据连续写入文件指定偏移处。
///
/// 与 writev 的区别：不依赖（也不修改）文件的当前偏移量，
/// 而是从 offset 处开始写入。多个 iov 条目连续写入：
/// offset, offset+len0, offset+len0+len1, ...
///
/// 语义细节（与 Linux pwritev 对齐）：
/// - 中途出错且已有部分数据写入时，返回已写字节数而非 -1
/// - 短写（write 返回不足请求长度）直接终止，不再处理后续 iov
pub fn sys_pwritev(
    fd: usize,
    iov_ptr: *const IoVec,
    iovcnt: usize,
    offset: isize,
) -> SysResult<usize> {
    const IOV_MAX: usize = 1024;
    if offset < 0 || iovcnt > IOV_MAX {
        return Err(Errno::EINVAL);
    }
    if iovcnt == 0 {
        return Ok(0);
    }

    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if !file.writable() {
        return Err(Errno::EBADF);
    }
    file.can_seek()?;

    let old_offset = file.get_offset(); // 保存原偏移
    file.seek(offset)?; // 定位到写入起点

    let mut total: usize = 0;
    for idx in 0..iovcnt {
        let mut item = IoVec {
            base: core::ptr::null_mut(),
            len: 0,
        };
        let iov_ret = unsafe { copy_from_user(&mut item as *mut IoVec, iov_ptr.add(idx), 1) };
        if let Err(err) = iov_ret {
            let _ = file.seek(old_offset as isize);
            return if total > 0 { Ok(total) } else { Err(err) };
        }
        if item.len == 0 {
            continue;
        }
        // 复用 sys_write 完成单段写入（内部走 file.write()，偏移自动推进）
        match sys_write(fd, item.base, item.len) {
            Ok(written) => {
                total = total.checked_add(written).ok_or(Errno::EINVAL)?;
                if written < item.len {
                    break; // 短写：文件空间不足，不再续写后续 iov
                }
            }
            Err(err) => {
                let _ = file.seek(old_offset as isize);
                return if total > 0 { Ok(total) } else { Err(err) };
            }
        }
    }

    let _ = file.seek(old_offset as isize); // 恢复原偏移
    Ok(total)
}

/// 系统调用 preadv2 — preadv 的扩展版本，增加 flags 参数。
///
/// offset == -1 时等价于 readv（使用文件当前偏移），
/// 否则等价于 preadv（从指定偏移读取）。
/// 当前内核不支持任何 flags（如 RWF_HIPRI/RWF_DSYNC 等）。
pub fn sys_preadv2(
    fd: usize,
    iov_ptr: *const IoVec,
    iovcnt: usize,
    offset: isize,
    flags: i32,
) -> SysResult<usize> {
    if flags != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    if offset == -1 {
        sys_readv(fd, iov_ptr, iovcnt)
    } else {
        sys_preadv(fd, iov_ptr, iovcnt, offset)
    }
}

/// 系统调用 pwritev2 — pwritev 的扩展版本，增加 flags 参数。
///
/// offset == -1 时等价于 writev（使用文件当前偏移），
/// 否则等价于 pwritev（从指定偏移写入）。
/// 当前内核不支持任何 flags。
pub fn sys_pwritev2(
    fd: usize,
    iov_ptr: *const IoVec,
    iovcnt: usize,
    offset: isize,
    flags: i32,
) -> SysResult<usize> {
    if flags != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    if offset == -1 {
        sys_writev(fd, iov_ptr, iovcnt)
    } else {
        sys_pwritev(fd, iov_ptr, iovcnt, offset)
    }
}
