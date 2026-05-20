// os/src/syscall/mm.rs

use super::{Errno, SysResult};
use crate::task::current_task;

/// 系统调用 sys-brk
/// TODO[UNIMPLEMENTED]: 需要补完 brk 逻辑。
pub fn sys_brk(addr: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-munmap
/// TODO[UNIMPLEMENTED]: 需要补完 munmap 逻辑。
pub fn sys_munmap(addr: usize, len: usize) -> SysResult<usize> {
    let _ = (addr, len);
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-mmap
/// TODO[UNIMPLEMENTED]: 需要补完 mmap 逻辑。
pub fn sys_mmap(
    addr: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: isize,
    offset: usize,
) -> SysResult<usize> {
    let _ = (addr, len, prot, flags, fd, offset);
    Err(Errno::ENOSYS)
}
