// os/src/syscall/time.rs

use super::SysResult;
use crate::mm::{copy_from_user, copy_to_user};
use crate::timer::get_time_ms;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Tms {
    pub tms_utime: usize,
    pub tms_stime: usize,
    pub tms_cutime: usize,
    pub tms_cstime: usize,
}

impl Default for Tms {
    fn default() -> Self {
        Self {
            tms_utime: 1,
            tms_stime: 1,
            tms_cutime: 1,
            tms_cstime: 1,
        }
    }
}

/// 系统调用 sys-times
///
/// TODO：目前只做固定实现
pub fn sys_times(buf: *mut Tms) -> SysResult<usize> {
    let tms = Tms::default();
    copy_to_user(buf, &tms as *const Tms, 1)?;
    Ok(0)
}

pub fn sys_gettimeofday(tv: *mut TimeVal, _tz: usize) -> SysResult<usize> {
    let ms = get_time_ms();
    let time_val = TimeVal {
        sec: ms / 1000,
        usec: (ms % 1000) * 1000,
    };
    copy_to_user(tv, &time_val as *const TimeVal, 1)?;
    Ok(0)
}

/// 系统调用 sys-nanosleep
///
/// TODO: 实现较简单，且未实现信号打断机制
pub fn sys_nanosleep(req: *const TimeVal, _rem: *mut TimeVal) -> SysResult<usize> {
    let mut time_val = TimeVal { sec: 0, usec: 0 };
    copy_from_user(&mut time_val as *mut TimeVal, req, 1)?;
    let time_ms = time_val.sec * 1000 + time_val.usec / 1000;

    let start_time = get_time_ms();
    loop {
        let current_time = get_time_ms();
        if current_time - start_time >= time_ms {
            // 插入气泡
            break;
        }
    }
    Ok(0)
}
