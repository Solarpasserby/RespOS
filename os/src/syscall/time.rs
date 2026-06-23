// os/src/syscall/time.rs

use super::{Errno, SysResult};
use crate::config::CLK_TCK;
use crate::mm::{copy_from_user, copy_to_user};
use crate::mutex::SpinLock;
use crate::signal::{SiField, Sig, SigInfo};
use crate::task::{TaskControlBlock, current_task, yield_current_task};
use crate::timer::{TimeSpec, get_time_ms, get_time_us, get_timeout_ms};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

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

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct ITimerSpec {
    pub interval: TimeSpec,
    pub value: TimeSpec,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct SigEvent {
    pub value: usize,
    pub signo: i32,
    pub notify: i32,
    pub pad: [i32; 12],
}

#[derive(Copy, Clone, Default)]
struct PosixTimer {
    owner_tgid: usize,
    clock_id: usize,
    signo: i32,
    deadline_ms: usize,
    interval_ms: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Timex {
    pub modes: u32,
    pub offset: isize,
    pub freq: isize,
    pub maxerror: isize,
    pub esterror: isize,
    pub status: i32,
    pub constant: isize,
    pub precision: isize,
    pub tolerance: isize,
    pub time: TimeVal,
    pub tick: isize,
    pub ppsfreq: isize,
    pub jitter: isize,
    pub shift: i32,
    pub stabil: isize,
    pub jitcnt: isize,
    pub calcnt: isize,
    pub errcnt: isize,
    pub stbcnt: isize,
    pub tai: i32,
    pub reserved: [i32; 11],
}

impl Timex {
    fn initial() -> Self {
        Self {
            precision: 1,
            tolerance: 32_768_000,
            tick: 1_000_000 / CLK_TCK as isize,
            ..Self::default()
        }
    }

    fn refresh_time(&mut self) {
        let us = get_time_us();
        self.time = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
}

lazy_static! {
    static ref TIMEX_STATE: SpinLock<Timex> = SpinLock::new(Timex::initial());
    static ref POSIX_TIMERS: SpinLock<BTreeMap<usize, PosixTimer>> = SpinLock::new(BTreeMap::new());
    static ref REALTIME_OFFSET_US: SpinLock<isize> = SpinLock::new(0);
}

static NEXT_POSIX_TIMER_ID: AtomicUsize = AtomicUsize::new(1);

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
        let us = realtime_us();
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
    const CLOCK_REALTIME_ALARM: usize = 8;

    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_REALTIME_ALARM {
        return Err(Errno::EINVAL);
    }

    let mut time_spec = TimeSpec::default();
    copy_from_user(&mut time_spec as *mut TimeSpec, tp, 1)?;
    if !time_spec.is_valid_duration() {
        return Err(Errno::EINVAL);
    }
    let task = current_task().expect("no current task");
    if task.euid() != 0 {
        return Err(Errno::EPERM);
    }

    let target_us = time_spec.checked_duration_us().ok_or(Errno::EINVAL)? as isize;
    let now_us = get_time_us() as isize;
    *REALTIME_OFFSET_US.lock() = target_us.saturating_sub(now_us);
    Ok(0)
}

fn realtime_us() -> usize {
    (get_time_us() as isize)
        .saturating_add(*REALTIME_OFFSET_US.lock())
        .max(0) as usize
}

fn timespec_from_us(us: usize) -> TimeSpec {
    TimeSpec {
        sec: (us / 1_000_000) as isize,
        nsec: ((us % 1_000_000) * 1000) as isize,
    }
}

pub fn clock_time_ms(clock_id: usize) -> SysResult<usize> {
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;
    const CLOCK_PROCESS_CPUTIME_ID: usize = 2;
    const CLOCK_THREAD_CPUTIME_ID: usize = 3;
    const CLOCK_MONOTONIC_RAW: usize = 4;
    const CLOCK_REALTIME_COARSE: usize = 5;
    const CLOCK_MONOTONIC_COARSE: usize = 6;
    const CLOCK_BOOTTIME: usize = 7;
    const CLOCK_REALTIME_ALARM: usize = 8;
    const CLOCK_BOOTTIME_ALARM: usize = 9;
    const CLOCK_TAI: usize = 11;

    match clock_id {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM => Ok(realtime_us() / 1000),
        CLOCK_MONOTONIC
        | CLOCK_PROCESS_CPUTIME_ID
        | CLOCK_THREAD_CPUTIME_ID
        | CLOCK_MONOTONIC_RAW
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_BOOTTIME
        | CLOCK_BOOTTIME_ALARM
        | CLOCK_TAI => Ok(get_time_ms()),
        _ => Err(Errno::EINVAL),
    }
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
    const CLOCK_REALTIME_ALARM: usize = 8;
    const CLOCK_BOOTTIME_ALARM: usize = 9;
    const CLOCK_TAI: usize = 11;

    // TODO[ABI-COMPAT]: 目前调度器还没有统计进程/线程 CPU 时间，
    // 所以 CPU 时间时钟暂时用墙上时钟近似。
    match clock_id {
        CLOCK_MONOTONIC
        | CLOCK_PROCESS_CPUTIME_ID
        | CLOCK_THREAD_CPUTIME_ID
        | CLOCK_MONOTONIC_RAW
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_BOOTTIME
        | CLOCK_BOOTTIME_ALARM
        | CLOCK_TAI => {
            let ms = get_time_ms();
            let time_spec = TimeSpec {
                sec: (ms / 1000) as isize,
                nsec: ((ms % 1000) * 1_000_000) as isize,
            };
            copy_to_user(tp, &time_spec as *const TimeSpec, 1)?;
            Ok(0)
        }
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM => {
            let time_spec = timespec_from_us(realtime_us());
            copy_to_user(tp, &time_spec as *const TimeSpec, 1)?;
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
}

fn is_supported_clock(clock_id: usize) -> bool {
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;
    const CLOCK_PROCESS_CPUTIME_ID: usize = 2;
    const CLOCK_THREAD_CPUTIME_ID: usize = 3;
    const CLOCK_MONOTONIC_RAW: usize = 4;
    const CLOCK_REALTIME_COARSE: usize = 5;
    const CLOCK_MONOTONIC_COARSE: usize = 6;
    const CLOCK_BOOTTIME: usize = 7;
    const CLOCK_REALTIME_ALARM: usize = 8;
    const CLOCK_BOOTTIME_ALARM: usize = 9;
    const CLOCK_TAI: usize = 11;

    matches!(
        clock_id,
        CLOCK_REALTIME
            | CLOCK_MONOTONIC
            | CLOCK_PROCESS_CPUTIME_ID
            | CLOCK_THREAD_CPUTIME_ID
            | CLOCK_MONOTONIC_RAW
            | CLOCK_REALTIME_COARSE
            | CLOCK_MONOTONIC_COARSE
            | CLOCK_BOOTTIME
            | CLOCK_REALTIME_ALARM
            | CLOCK_BOOTTIME_ALARM
            | CLOCK_TAI
    )
}

pub fn sys_clock_getres(clock_id: usize, res: *mut TimeSpec) -> SysResult<usize> {
    if !is_supported_clock(clock_id) {
        return Err(Errno::EINVAL);
    }
    if !res.is_null() {
        let time_spec = TimeSpec { sec: 0, nsec: 1 };
        copy_to_user(res, &time_spec as *const TimeSpec, 1)?;
    }
    Ok(0)
}

fn validate_timex(new_timer: &Timex) -> SysResult<()> {
    const ADJ_TICK: u32 = 0x4000;

    if new_timer.modes & ADJ_TICK != 0 {
        let low = 900_000 / CLK_TCK as isize;
        let high = 1_100_000 / CLK_TCK as isize;
        if new_timer.tick < low || new_timer.tick > high {
            return Err(Errno::EINVAL);
        }
    }
    Ok(())
}

fn apply_timex_update(state: &mut Timex, new_timer: Timex) {
    const ADJ_OFFSET: u32 = 0x0001;
    const ADJ_FREQUENCY: u32 = 0x0002;
    const ADJ_MAXERROR: u32 = 0x0004;
    const ADJ_ESTERROR: u32 = 0x0008;
    const ADJ_STATUS: u32 = 0x0010;
    const ADJ_TIMECONST: u32 = 0x0020;
    const ADJ_TAI: u32 = 0x0080;
    const ADJ_TICK: u32 = 0x4000;

    let modes = new_timer.modes;
    if modes & ADJ_OFFSET != 0 {
        state.offset = new_timer.offset;
    }
    if modes & ADJ_FREQUENCY != 0 {
        state.freq = new_timer.freq;
    }
    if modes & ADJ_MAXERROR != 0 {
        state.maxerror = new_timer.maxerror;
    }
    if modes & ADJ_ESTERROR != 0 {
        state.esterror = new_timer.esterror;
    }
    if modes & ADJ_STATUS != 0 {
        state.status = new_timer.status;
    }
    if modes & ADJ_TIMECONST != 0 {
        state.constant = new_timer.constant;
    }
    if modes & ADJ_TAI != 0 {
        state.tai = new_timer.tai;
    }
    if modes & ADJ_TICK != 0 {
        state.tick = new_timer.tick;
    }
}

pub fn sys_adjtimex(buf: *mut Timex) -> SysResult<usize> {
    let mut new_timer = Timex::default();
    copy_from_user(&mut new_timer as *mut Timex, buf, 1)?;

    let task = current_task().expect("no current task");
    if new_timer.modes != 0 && task.euid() != 0 {
        return Err(Errno::EPERM);
    }
    validate_timex(&new_timer)?;

    let mut state = TIMEX_STATE.lock();
    apply_timex_update(&mut state, new_timer);
    state.modes = new_timer.modes;
    state.refresh_time();
    let current = *state;
    copy_to_user(buf, &current as *const Timex, 1)?;
    Ok(0)
}

pub fn sys_clock_adjtime(clock_id: usize, buf: *mut Timex) -> SysResult<usize> {
    const CLOCK_REALTIME: usize = 0;

    if clock_id != CLOCK_REALTIME {
        return Err(Errno::EINVAL);
    }
    sys_adjtimex(buf)
}

fn timespec_to_ms(ts: TimeSpec) -> SysResult<usize> {
    ts.checked_duration_ms().ok_or(Errno::EINVAL)
}

fn ms_to_timespec(ms: usize) -> TimeSpec {
    TimeSpec {
        sec: (ms / 1000) as isize,
        nsec: ((ms % 1000) * 1_000_000) as isize,
    }
}

fn posix_timer_remaining_ms(timer: &PosixTimer) -> usize {
    if timer.deadline_ms == 0 {
        0
    } else {
        timer
            .deadline_ms
            .saturating_sub(clock_time_ms(timer.clock_id).unwrap_or(usize::MAX))
    }
}

fn posix_timer_snapshot(timer: &PosixTimer) -> ITimerSpec {
    ITimerSpec {
        interval: ms_to_timespec(timer.interval_ms),
        value: ms_to_timespec(posix_timer_remaining_ms(timer)),
    }
}

pub fn sys_timer_create(
    clock_id: usize,
    sevp: *const SigEvent,
    timerid: *mut i32,
) -> SysResult<usize> {
    if !is_supported_clock(clock_id) {
        return Err(Errno::EINVAL);
    }

    let signo = if sevp.is_null() {
        Sig::SIGALRM.raw()
    } else {
        let mut event = SigEvent::default();
        copy_from_user(&mut event as *mut SigEvent, sevp, 1)?;
        if event.notify != 0 {
            return Err(Errno::EINVAL);
        }
        if !Sig::from(event.signo).is_valid() {
            return Err(Errno::EINVAL);
        }
        event.signo
    };

    let task = current_task().expect("no current task");
    let id = NEXT_POSIX_TIMER_ID.fetch_add(1, Ordering::Relaxed) as i32;
    let timer = PosixTimer {
        owner_tgid: task.tgid(),
        clock_id,
        signo,
        deadline_ms: 0,
        interval_ms: 0,
    };
    POSIX_TIMERS.lock().insert(id as usize, timer);
    copy_to_user(timerid, &id as *const i32, 1)?;
    Ok(0)
}

pub fn sys_timer_delete(timerid: usize) -> SysResult<usize> {
    let task = current_task().expect("no current task");
    let removed = {
        let mut timers = POSIX_TIMERS.lock();
        match timers.get(&timerid) {
            Some(timer) if timer.owner_tgid == task.tgid() => timers.remove(&timerid),
            _ => None,
        }
    };
    if removed.is_some() {
        Ok(0)
    } else {
        Err(Errno::EINVAL)
    }
}

pub fn sys_timer_getoverrun(timerid: usize) -> SysResult<usize> {
    let task = current_task().expect("no current task");
    let timers = POSIX_TIMERS.lock();
    match timers.get(&timerid) {
        Some(timer) if timer.owner_tgid == task.tgid() => Ok(0),
        _ => Err(Errno::EINVAL),
    }
}

pub fn sys_timer_gettime(timerid: usize, curr_value: *mut ITimerSpec) -> SysResult<usize> {
    let task = current_task().expect("no current task");
    let current = {
        let timers = POSIX_TIMERS.lock();
        let timer = timers.get(&timerid).ok_or(Errno::EINVAL)?;
        if timer.owner_tgid != task.tgid() {
            return Err(Errno::EINVAL);
        }
        posix_timer_snapshot(timer)
    };
    copy_to_user(curr_value, &current as *const ITimerSpec, 1)?;
    Ok(0)
}

pub fn sys_timer_settime(
    timerid: usize,
    flags: usize,
    new_value: *const ITimerSpec,
    old_value: *mut ITimerSpec,
) -> SysResult<usize> {
    const TIMER_ABSTIME: usize = 1;

    if new_value.is_null() {
        return Err(Errno::EINVAL);
    }
    if flags & !TIMER_ABSTIME != 0 {
        return Err(Errno::EINVAL);
    }

    let mut new_timer = ITimerSpec::default();
    copy_from_user(&mut new_timer as *mut ITimerSpec, new_value, 1)?;
    let value_ms = timespec_to_ms(new_timer.value)?;
    let interval_ms = timespec_to_ms(new_timer.interval)?;

    let task = current_task().expect("no current task");
    let old = {
        let mut timers = POSIX_TIMERS.lock();
        let timer = timers.get_mut(&timerid).ok_or(Errno::EINVAL)?;
        if timer.owner_tgid != task.tgid() {
            return Err(Errno::EINVAL);
        }
        let old = posix_timer_snapshot(timer);
        let now_ms = clock_time_ms(timer.clock_id)?;
        timer.deadline_ms = if value_ms == 0 {
            0
        } else if flags & TIMER_ABSTIME != 0 {
            value_ms.max(now_ms)
        } else {
            now_ms.saturating_add(value_ms)
        };
        timer.interval_ms = interval_ms;
        old
    };

    if !old_value.is_null() {
        copy_to_user(old_value, &old as *const ITimerSpec, 1)?;
    }
    Ok(0)
}

pub fn check_posix_timers(task: &TaskControlBlock) {
    let mut expired = Vec::new();
    {
        let mut timers = POSIX_TIMERS.lock();
        for timer in timers.values_mut() {
            let Ok(now_ms) = clock_time_ms(timer.clock_id) else {
                continue;
            };
            if timer.owner_tgid != task.tgid()
                || timer.deadline_ms == 0
                || now_ms < timer.deadline_ms
            {
                continue;
            }
            expired.push(timer.signo);
            timer.deadline_ms = if timer.interval_ms == 0 {
                0
            } else {
                now_ms.saturating_add(timer.interval_ms)
            };
        }
    }

    for signo in expired {
        let sig = Sig::from(signo);
        if sig.is_valid() {
            let siginfo = SigInfo::new(sig.raw(), SigInfo::KERNEL, SiField::None);
            task.receive_siginfo(siginfo, false);
        }
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
    if which > 2 {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");
    let current = ITimerVal {
        interval: ms_to_timeval(task.itimer_interval_ms(which)),
        value: ms_to_timeval(task.itimer_remaining_ms(which)),
    };
    copy_to_user(curr_value, &current as *const ITimerVal, 1)?;
    Ok(0)
}

pub fn sys_setitimer(
    which: usize,
    new_value: *const ITimerVal,
    old_value: *mut ITimerVal,
) -> SysResult<usize> {
    if which > 2 {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");
    if !old_value.is_null() {
        let old = ITimerVal {
            interval: ms_to_timeval(task.itimer_interval_ms(which)),
            value: ms_to_timeval(task.itimer_remaining_ms(which)),
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
    task.set_itimer_ms(which, value_ms, interval_ms);
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
