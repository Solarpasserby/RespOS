// os/src/syscall/time.rs

use super::{Errno, SysResult};
use crate::mm::{copy_from_user, copy_to_user};
use crate::task::{current_task, yield_current_task};
use crate::timer::{TimeSpec, get_time_ms, get_time_us, get_timeout_ms};

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct TimeZone {
    pub minuteswest: i32,
    pub dsttime: i32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct ITimerVal {
    pub interval: TimeVal,
    pub value: TimeVal,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Tms {
    pub tms_utime: usize,
    pub tms_stime: usize,
    pub tms_cutime: usize,
    pub tms_cstime: usize,
}

/// 系统调用 sys-times。
///
/// TODO[ABI-COMPAT]: 当前先用 wall-clock 近似 user/system CPU tick，后续应替换为
/// 调度器实际运行时间记账。
pub fn sys_times(buf: *mut Tms) -> SysResult<usize> {
    let task = current_task().expect("no current task");
    let ticks = task.elapsed_ticks();
    let (child_utime, child_stime) = task.child_ticks();
    let tms = Tms {
        tms_utime: ticks,
        tms_stime: ticks,
        tms_cutime: child_utime,
        tms_cstime: child_stime,
    };
    copy_to_user(buf, &tms as *const Tms, 1)?;
    Ok(ticks)
}

pub fn sys_gettimeofday(tv: *mut TimeVal, tz: *mut TimeZone) -> SysResult<usize> {
    if !tv.is_null() {
        let us = get_time_us();
        let time_val = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
        copy_to_user(tv, &time_val as *const TimeVal, 1)?;
    }
    if !tz.is_null() {
        // Linux 仍接受历史遗留的 timezone 参数；系统时区固定为 UTC。
        let time_zone = TimeZone {
            minuteswest: 0,
            dsttime: 0,
        };
        copy_to_user(tz, &time_zone as *const TimeZone, 1)?;
    }
    Ok(0)
}

pub fn sys_settimeofday(tv: *const TimeVal, _tz: *const TimeZone) -> SysResult<usize> {
    if !tv.is_null() {
        let mut time_val = TimeVal::default();
        copy_from_user(&mut time_val as *mut TimeVal, tv, 1)?;
        if (time_val.sec as isize) < 0 || time_val.usec >= 1_000_000 {
            return Err(Errno::EINVAL);
        }
    }
    Err(Errno::EPERM)
}

pub fn sys_clock_settime(clock_id: usize, tp: *const TimeSpec) -> SysResult<usize> {
    const CLOCK_REALTIME: usize = 0;

    if clock_id != CLOCK_REALTIME {
        return Err(Errno::EINVAL);
    }

    let mut time_spec = TimeSpec::default();
    copy_from_user(&mut time_spec as *mut TimeSpec, tp, 1)?;
    if !time_spec.is_valid_duration() {
        return Err(Errno::EINVAL);
    }

    Err(Errno::EPERM)
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

fn timeval_to_ms(tv: TimeVal) -> SysResult<usize> {
    if tv.usec >= 1_000_000 {
        return Err(Errno::EINVAL);
    }
    tv.sec
        .checked_mul(1000)
        .and_then(|ms| ms.checked_add(tv.usec.div_ceil(1000)))
        .ok_or(Errno::EINVAL)
}

fn ms_to_timeval(ms: usize) -> TimeVal {
    TimeVal {
        sec: ms / 1000,
        usec: (ms % 1000) * 1000,
    }
}

pub fn sys_getitimer(which: usize, curr_value: *mut ITimerVal) -> SysResult<usize> {
    const ITIMER_REAL: usize = 0;
    if which != ITIMER_REAL {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");
    let current = ITimerVal {
        interval: ms_to_timeval(task.real_timer_interval_ms()),
        value: ms_to_timeval(task.real_timer_remaining_ms()),
    };
    copy_to_user(curr_value, &current as *const ITimerVal, 1)?;
    Ok(0)
}

pub fn sys_setitimer(
    which: usize,
    new_value: *const ITimerVal,
    old_value: *mut ITimerVal,
) -> SysResult<usize> {
    const ITIMER_REAL: usize = 0;
    if which != ITIMER_REAL {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");
    if !old_value.is_null() {
        let old = ITimerVal {
            interval: ms_to_timeval(task.real_timer_interval_ms()),
            value: ms_to_timeval(task.real_timer_remaining_ms()),
        };
        copy_to_user(old_value, &old as *const ITimerVal, 1)?;
    }

    if new_value.is_null() {
        return Err(Errno::EFAULT);
    }
    let mut new_timer = ITimerVal::default();
    copy_from_user(&mut new_timer as *mut ITimerVal, new_value, 1)?;
    let value_ms = timeval_to_ms(new_timer.value)?;
    let interval_ms = timeval_to_ms(new_timer.interval)?;
    task.set_real_timer_ms(value_ms, interval_ms);
    Ok(0)
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

pub fn sys_clock_nanosleep(
    clock_id: usize,
    flags: usize,
    req: *const TimeSpec,
    rem: *mut TimeSpec,
) -> SysResult<usize> {
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;
    const TIMER_ABSTIME: usize = 1;

    // glibc 的 nanosleep() 在 RISC-V/LoongArch 上会通过 clock_nanosleep()
    // 进入内核。这里先覆盖 LTP 当前使用的相对睡眠语义；绝对时间睡眠需要
    // 基于 clock_id 计算 deadline，不能简单复用 sys_nanosleep。
    match clock_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC => {}
        _ => return Err(Errno::EINVAL),
    }
    if flags & TIMER_ABSTIME != 0 {
        return Err(Errno::EINVAL);
    }
    sys_nanosleep(req, rem)
}
