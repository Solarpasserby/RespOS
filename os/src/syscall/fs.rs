// os/src/syscall/fs.rs

use crate::task::{current_task};
use super::{SysResult, Errno};

/// 系统调用 sys-read
pub fn sys_read(fd: usize, buf: *mut u8, len: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_file(fd)?;
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
    let file = task.get_file(fd)?;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let ret = file.write(unsafe {
        core::slice::from_raw_parts_mut(buf, len)
    })?;
    Ok(ret)
}
