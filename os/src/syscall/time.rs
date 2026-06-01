// os/src/syscall/time.rs

use super::{Errno, SysResult};
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
pub struct TimeSpec {
    pub sec: usize,
    pub nsec: usize,
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

pub fn sys_clock_gettime(clock_id: usize, tp: *mut TimeSpec) -> SysResult<usize> {
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;
    const CLOCK_PROCESS_CPUTIME_ID: usize = 2;
    const CLOCK_THREAD_CPUTIME_ID: usize = 3;
    const CLOCK_MONOTONIC_RAW: usize = 4;
    const CLOCK_REALTIME_COARSE: usize = 5;
    const CLOCK_MONOTONIC_COARSE: usize = 6;
    const CLOCK_BOOTTIME: usize = 7;

    // TODO[ABI-COMPAT]: 目前调度器还没有统计进程/线程 CPU 时间，
    // 所以 CPU 时间时钟暂时用墙上时钟近似。
    match clock_id {
        CLOCK_REALTIME
        | CLOCK_MONOTONIC
        | CLOCK_PROCESS_CPUTIME_ID
        | CLOCK_THREAD_CPUTIME_ID
        | CLOCK_MONOTONIC_RAW
        | CLOCK_REALTIME_COARSE
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_BOOTTIME => {
            let ms = get_time_ms();
            let time_spec = TimeSpec {
                sec: ms / 1000,
                nsec: (ms % 1000) * 1_000_000,
            };
            copy_to_user(tp, &time_spec as *const TimeSpec, 1)?;
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
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
