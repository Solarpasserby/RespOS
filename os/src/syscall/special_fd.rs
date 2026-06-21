use super::{Errno, SysResult};
use crate::fs::vfs::InodeType;
use crate::fs::{FdEntry, OpenFlags, SpecialFd};
use crate::mm::{check_user_readable, copy_cstr_from_user};
use crate::task::{TASK_MANAGER, current_task};
use alloc::sync::Arc;

const O_NONBLOCK: usize = OpenFlags::O_NONBLOCK.bits() as usize;
const O_CLOEXEC: usize = OpenFlags::O_CLOEXEC.bits() as usize;

const MFD_CLOEXEC: usize = 0x0001;
const MFD_ALLOW_SEALING: usize = 0x0002;
const MFD_HUGETLB: usize = 0x0004;
const MFD_HUGE_MASK: usize = 0x3f << 26;
const MFD_ALLOWED_FLAGS: usize = MFD_CLOEXEC | MFD_ALLOW_SEALING | MFD_HUGETLB | MFD_HUGE_MASK;

const PIDFD_NONBLOCK: usize = O_NONBLOCK;

fn alloc_special_fd(flags: OpenFlags) -> SysResult<usize> {
    alloc_special_fd_with_type(flags, InodeType::Unknown)
}

fn alloc_special_fd_with_type(flags: OpenFlags, ty: InodeType) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = Arc::new(SpecialFd::new(flags, ty));
    task.alloc_fd(FdEntry::new(file, flags))
}

fn fd_flags(nonblock: bool, cloexec: bool) -> OpenFlags {
    let mut flags = OpenFlags::O_RDWR;
    if nonblock {
        flags |= OpenFlags::O_NONBLOCK;
    }
    if cloexec {
        flags |= OpenFlags::O_CLOEXEC;
    }
    flags
}

fn flags_from_o_flags(flags: usize, allowed: usize) -> SysResult<OpenFlags> {
    if flags & !allowed != 0 {
        return Err(Errno::EINVAL);
    }
    Ok(fd_flags(flags & O_NONBLOCK != 0, flags & O_CLOEXEC != 0))
}

pub fn sys_eventfd2(_initval: usize, flags: usize) -> SysResult<usize> {
    let flags = flags_from_o_flags(flags, O_NONBLOCK | O_CLOEXEC)?;
    alloc_special_fd(flags)
}

pub fn sys_epoll_create1(flags: usize) -> SysResult<usize> {
    let flags = flags_from_o_flags(flags, O_CLOEXEC)?;
    alloc_special_fd(flags)
}

pub fn sys_inotify_init1(flags: usize) -> SysResult<usize> {
    let flags = flags_from_o_flags(flags, O_NONBLOCK | O_CLOEXEC)?;
    alloc_special_fd(flags)
}

pub fn sys_signalfd4(
    fd: isize,
    mask: *const u8,
    _sizemask: usize,
    flags: usize,
) -> SysResult<usize> {
    if fd != -1 {
        return Err(Errno::EINVAL);
    }
    if !mask.is_null() {
        check_user_readable(mask, 1)?;
    }
    let flags = flags_from_o_flags(flags, O_NONBLOCK | O_CLOEXEC)?;
    alloc_special_fd(flags)
}

pub fn sys_timerfd_create(_clockid: usize, flags: usize) -> SysResult<usize> {
    let flags = flags_from_o_flags(flags, O_NONBLOCK | O_CLOEXEC)?;
    alloc_special_fd(flags)
}

pub fn sys_pidfd_open(pid: usize, flags: usize) -> SysResult<usize> {
    if flags & !PIDFD_NONBLOCK != 0 {
        return Err(Errno::EINVAL);
    }
    if pid == 0 {
        return Err(Errno::EINVAL);
    }
    if TASK_MANAGER.get(pid).is_none() {
        return Err(Errno::ESRCH);
    }
    alloc_special_fd(fd_flags(flags & PIDFD_NONBLOCK != 0, true))
}

pub fn sys_fanotify_init(flags: usize, _event_f_flags: usize) -> SysResult<usize> {
    let flags = flags_from_o_flags(flags, O_NONBLOCK | O_CLOEXEC)?;
    alloc_special_fd(flags)
}

pub fn sys_userfaultfd(flags: usize) -> SysResult<usize> {
    let flags = flags_from_o_flags(flags, O_NONBLOCK | O_CLOEXEC)?;
    alloc_special_fd(flags)
}

pub fn sys_perf_event_open(
    attr: *const u8,
    _pid: isize,
    _cpu: isize,
    _group_fd: isize,
    _flags: usize,
) -> SysResult<usize> {
    if attr.is_null() {
        return Err(Errno::EFAULT);
    }
    check_user_readable(attr, 1)?;
    alloc_special_fd(OpenFlags::O_RDWR)
}

pub fn sys_io_uring_setup(entries: usize, params: *const u8) -> SysResult<usize> {
    if entries == 0 {
        return Err(Errno::EINVAL);
    }
    if params.is_null() {
        return Err(Errno::EFAULT);
    }
    check_user_readable(params, 1)?;
    alloc_special_fd(OpenFlags::O_RDWR)
}

pub fn sys_bpf(cmd: usize, attr: *const u8, size: usize) -> SysResult<usize> {
    const BPF_MAP_CREATE: usize = 0;
    if cmd != BPF_MAP_CREATE {
        return Err(Errno::EINVAL);
    }
    if attr.is_null() || size == 0 {
        return Err(Errno::EFAULT);
    }
    check_user_readable(attr, 1)?;
    alloc_special_fd(OpenFlags::O_RDWR)
}

pub fn sys_fsopen(fs_name: *const u8, flags: usize) -> SysResult<usize> {
    const FSOPEN_CLOEXEC: usize = 0x0000_0001;
    if flags & !FSOPEN_CLOEXEC != 0 {
        return Err(Errno::EINVAL);
    }
    let _ = copy_cstr_from_user(fs_name)?;
    alloc_special_fd(fd_flags(false, flags & FSOPEN_CLOEXEC != 0))
}

pub fn sys_fspick(_dfd: isize, path: *const u8, flags: usize) -> SysResult<usize> {
    const FSPICK_CLOEXEC: usize = 0x0000_0001;
    if flags & !FSPICK_CLOEXEC != 0 {
        return Err(Errno::EINVAL);
    }
    let _ = copy_cstr_from_user(path)?;
    alloc_special_fd(fd_flags(false, flags & FSPICK_CLOEXEC != 0))
}

pub fn sys_open_tree(_dfd: isize, path: *const u8, flags: usize) -> SysResult<usize> {
    const OPEN_TREE_CLOEXEC: usize = 0x0000_0001;
    const OPEN_TREE_CLONE: usize = 0x0000_0002;
    const AT_EMPTY_PATH: usize = 0x1000;
    const AT_RECURSIVE: usize = 0x8000;
    const ALLOWED: usize = OPEN_TREE_CLOEXEC | OPEN_TREE_CLONE | AT_EMPTY_PATH | AT_RECURSIVE;
    if flags & !ALLOWED != 0 {
        return Err(Errno::EINVAL);
    }
    let _ = copy_cstr_from_user(path)?;
    let flags = fd_flags(false, flags & OPEN_TREE_CLOEXEC != 0) | OpenFlags::O_PATH;
    alloc_special_fd(flags)
}

pub fn sys_memfd_create(name: *const u8, flags: usize) -> SysResult<usize> {
    if flags & !MFD_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    let _ = copy_cstr_from_user(name)?;
    alloc_special_fd_with_type(
        fd_flags(false, flags & MFD_CLOEXEC != 0),
        InodeType::Regular,
    )
}

pub fn sys_memfd_secret(flags: usize) -> SysResult<usize> {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }
    alloc_special_fd_with_type(OpenFlags::O_RDWR, InodeType::Regular)
}
