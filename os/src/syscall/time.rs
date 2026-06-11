// os/src/syscall/time.rs

use super::{Errno, SysResult};
use crate::mm::{copy_from_user, copy_to_user};
use crate::task::{current_task, yield_current_task};
use crate::timer::{TimeSpec, get_time_ms, get_time_us, get_timeout_ms};

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
    let us = get_time_us();
    let time_val = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
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
                sec: (ms / 1000) as isize,
                nsec: ((ms % 1000) * 1_000_000) as isize,
            };
            copy_to_user(tp, &time_spec as *const TimeSpec, 1)?;
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
}

/// 系统调用 sys-nanosleep
///
pub fn sys_nanosleep(req: *const TimeSpec, rem: *mut TimeSpec) -> SysResult<usize> {
    let mut req_time = TimeSpec::default();
    copy_from_user(&mut req_time as *mut TimeSpec, req, 1)?;
    let time_ms = req_time.checked_duration_ms().ok_or(Errno::EINVAL)?;
    let task = current_task().expect("no current task");

    let start_time = get_timeout_ms();
    loop {
        let current_time = get_timeout_ms();
        let elapsed = current_time.saturating_sub(start_time);
        if elapsed >= time_ms {
            break;
        }
        task.set_interruptible(true);
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            task.set_interruptible(false);
            if !rem.is_null() {
                let left_ms = time_ms - elapsed;
                let remain = TimeSpec {
                    sec: (left_ms / 1000) as isize,
                    nsec: ((left_ms % 1000) * 1_000_000) as isize,
                };
                copy_to_user(rem, &remain as *const TimeSpec, 1)?;
            }
            return Err(Errno::EINTR);
        }
        yield_current_task();
        task.set_interruptible(false);
        if task.is_interrupted() || task.check_signal_interrupt() {
            task.clear_interrupted();
            if !rem.is_null() {
                let now = get_timeout_ms();
                let elapsed = now.saturating_sub(start_time).min(time_ms);
                let left_ms = time_ms - elapsed;
                let remain = TimeSpec {
                    sec: (left_ms / 1000) as isize,
                    nsec: ((left_ms % 1000) * 1_000_000) as isize,
                };
                copy_to_user(rem, &remain as *const TimeSpec, 1)?;
            }
            return Err(Errno::EINTR);
        }
    }
    Ok(0)
}
