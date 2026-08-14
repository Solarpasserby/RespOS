// os/src/syscall/time.rs

use super::{Errno, SysResult};
use crate::config::CLK_TCK;
use crate::mm::{copy_from_user, copy_to_user};
use crate::mutex::SpinLock;
use crate::signal::{SiField, Sig, SigInfo};
use crate::task::{
    CpuClockHandle, TASK_MANAGER, current_task, prepare_current_task_blocked, switch_to_next_task,
    yield_current_task,
};
use crate::timer::{TimeSpec, get_timeout_us};
use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

const CAP_SYS_TIME: usize = 25;

const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;
const CLOCK_PROCESS_CPUTIME_ID: usize = 2;
const CLOCK_THREAD_CPUTIME_ID: usize = 3;
const CLOCK_MONOTONIC_RAW: usize = 4;
const CLOCK_REALTIME_COARSE: usize = 5;
const CLOCK_MONOTONIC_COARSE: usize = 6;
const CLOCK_BOOTTIME: usize = 7;

const CLOCK_FINE_RESOLUTION_NS: isize = 1_000;
const CLOCK_COARSE_RESOLUTION_NS: isize = 1_000_000;
#[cfg(feature = "debug_traces")]
const TIMER_LIFECYCLE_TRACE: bool = option_env!("TASK_A_TIMER_LIFECYCLE_TRACE").is_some();

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

#[derive(Clone, Default)]
struct PosixTimer {
    owner_tgid: usize,
    owner: Weak<crate::task::TaskControlBlock>,
    clock_id: usize,
    cpu_clock: Option<CpuClockHandle>,
    signo: i32,
    deadline_ms: usize,
    interval_ms: usize,
}

#[derive(Clone, Copy)]
struct NanosleepWait {
    clock_id: usize,
    deadline_us: usize,
    timed_out: bool,
}

struct NanosleepWaits {
    waits: BTreeMap<usize, NanosleepWait>,
    deadlines: BTreeMap<usize, BTreeMap<usize, Vec<usize>>>,
}

impl NanosleepWaits {
    fn new() -> Self {
        Self {
            waits: BTreeMap::new(),
            deadlines: BTreeMap::new(),
        }
    }

    fn register(&mut self, tid: usize, clock_id: usize, deadline_us: usize) {
        self.waits.insert(
            tid,
            NanosleepWait {
                clock_id,
                deadline_us,
                timed_out: false,
            },
        );
        self.deadlines
            .entry(clock_id)
            .or_default()
            .entry(deadline_us)
            .or_default()
            .push(tid);
    }

    fn finish(&mut self, tid: usize) -> bool {
        let Some(wait) = self.waits.remove(&tid) else {
            return false;
        };
        let mut remove_clock = false;
        if let Some(deadlines) = self.deadlines.get_mut(&wait.clock_id) {
            let remove_bucket = if let Some(tids) = deadlines.get_mut(&wait.deadline_us) {
                tids.retain(|queued_tid| *queued_tid != tid);
                tids.is_empty()
            } else {
                false
            };
            if remove_bucket {
                deadlines.remove(&wait.deadline_us);
            }
            remove_clock = deadlines.is_empty();
        }
        if remove_clock {
            self.deadlines.remove(&wait.clock_id);
        }
        wait.timed_out
    }
}

lazy_static! {
    static ref NANOSLEEP_WAITS: SpinLock<NanosleepWaits> = SpinLock::new(NanosleepWaits::new());
}

const NANOSLEEP_TIMEOUT_CLOCK: usize = usize::MAX;

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
        let us = realtime_us();
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
/// User/system time are not split yet, but the total is now derived from
/// scheduler runtime rather than wall-clock lifetime.
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
        let task = current_task().expect("no current task");
        if !task.has_cap(CAP_SYS_TIME) {
            return Err(Errno::EPERM);
        }
        let target_us = time_val
            .sec
            .checked_mul(1_000_000)
            .and_then(|us| us.checked_add(time_val.usec))
            .and_then(|us| isize::try_from(us).ok())
            .ok_or(Errno::EINVAL)?;
        *REALTIME_OFFSET_US.lock() = target_us.saturating_sub(monotonic_us() as isize);
        return Ok(0);
    }
    if !_tz.is_null()
        && !current_task()
            .expect("no current task")
            .has_cap(CAP_SYS_TIME)
    {
        return Err(Errno::EPERM);
    }
    Ok(0)
}

pub fn sys_clock_settime(clock_id: usize, tp: *const TimeSpec) -> SysResult<usize> {
    if clock_id != CLOCK_REALTIME {
        return Err(Errno::EINVAL);
    }

    let mut time_spec = TimeSpec::default();
    copy_from_user(&mut time_spec as *mut TimeSpec, tp, 1)?;
    if !time_spec.is_valid_duration() {
        return Err(Errno::EINVAL);
    }
    let task = current_task().expect("no current task");
    if !task.has_cap(CAP_SYS_TIME) {
        return Err(Errno::EPERM);
    }

    let target_us = isize::try_from(time_spec.checked_duration_us().ok_or(Errno::EINVAL)?)
        .map_err(|_| Errno::EINVAL)?;
    let now_us = monotonic_us() as isize;
    *REALTIME_OFFSET_US.lock() = target_us.saturating_sub(now_us);
    Ok(0)
}

fn monotonic_us() -> usize {
    get_timeout_us()
}

fn realtime_us() -> usize {
    (monotonic_us() as isize)
        .saturating_add(*REALTIME_OFFSET_US.lock())
        .max(0) as usize
}

fn timespec_from_us(us: usize) -> TimeSpec {
    TimeSpec {
        sec: (us / 1_000_000) as isize,
        nsec: ((us % 1_000_000) * 1000) as isize,
    }
}

fn clock_time_us(clock_id: usize) -> SysResult<usize> {
    match clock_id {
        CLOCK_REALTIME => Ok(realtime_us()),
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_BOOTTIME => Ok(monotonic_us()),
        CLOCK_PROCESS_CPUTIME_ID => Ok(current_task().ok_or(Errno::ESRCH)?.process_cpu_time_us()),
        CLOCK_THREAD_CPUTIME_ID => Ok(current_task().ok_or(Errno::ESRCH)?.thread_cpu_time_us()),
        CLOCK_REALTIME_COARSE => Ok(realtime_us() / 1000 * 1000),
        CLOCK_MONOTONIC_COARSE => Ok(monotonic_us() / 1000 * 1000),
        _ => Err(Errno::EINVAL),
    }
}

pub fn clock_time_ms(clock_id: usize) -> SysResult<usize> {
    Ok(clock_time_us(clock_id)? / 1000)
}

pub fn sys_clock_gettime(clock_id: usize, tp: *mut TimeSpec) -> SysResult<usize> {
    let time_spec = timespec_from_us(clock_time_us(clock_id)?);
    copy_to_user(tp, &time_spec as *const TimeSpec, 1)?;
    Ok(0)
}

fn is_readable_clock(clock_id: usize) -> bool {
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
    )
}

fn is_nanosleep_clock(clock_id: usize) -> bool {
    matches!(clock_id, CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_BOOTTIME)
}

fn is_posix_timer_clock(clock_id: usize) -> bool {
    matches!(
        clock_id,
        CLOCK_REALTIME
            | CLOCK_MONOTONIC
            | CLOCK_PROCESS_CPUTIME_ID
            | CLOCK_THREAD_CPUTIME_ID
            | CLOCK_BOOTTIME
    )
}

fn register_nanosleep_wait(tid: usize, clock_id: usize, deadline_us: usize) {
    NANOSLEEP_WAITS.lock().register(tid, clock_id, deadline_us);
    if let Ok(now_us) = nanosleep_clock_us(clock_id) {
        crate::timer::request_task_timer_after_us(deadline_us.saturating_sub(now_us));
    }
}

fn finish_nanosleep_wait(tid: usize) -> bool {
    NANOSLEEP_WAITS.lock().finish(tid)
}

pub fn register_task_timeout(tid: usize, deadline_ms: usize) {
    register_task_timeout_us(tid, deadline_ms.saturating_mul(1000));
}

pub fn register_task_timeout_us(tid: usize, deadline_us: usize) {
    register_nanosleep_wait(tid, NANOSLEEP_TIMEOUT_CLOCK, deadline_us);
}

pub fn finish_task_timeout(tid: usize) -> bool {
    finish_nanosleep_wait(tid)
}

fn nanosleep_clock_us(clock_id: usize) -> SysResult<usize> {
    if clock_id == NANOSLEEP_TIMEOUT_CLOCK {
        Ok(get_timeout_us())
    } else {
        clock_time_us(clock_id)
    }
}

pub fn check_nanosleep_timeouts() {
    let mut expired = Vec::new();
    let next_deadlines = {
        let mut waits = NANOSLEEP_WAITS.lock();
        let clock_ids: Vec<usize> = waits.deadlines.keys().copied().collect();
        for clock_id in clock_ids {
            let Ok(now_us) = nanosleep_clock_us(clock_id) else {
                continue;
            };
            let Some(deadlines) = waits.deadlines.get_mut(&clock_id) else {
                continue;
            };
            let mut due = Vec::new();
            while let Some((&deadline_us, _)) = deadlines.first_key_value() {
                if deadline_us > now_us {
                    break;
                }
                due.push((
                    deadline_us,
                    deadlines.remove(&deadline_us).unwrap_or_default(),
                ));
            }
            if deadlines.is_empty() {
                waits.deadlines.remove(&clock_id);
            }
            for (deadline_us, tids) in due {
                for tid in tids {
                    let Some(wait) = waits.waits.get_mut(&tid) else {
                        continue;
                    };
                    if wait.clock_id == clock_id
                        && wait.deadline_us == deadline_us
                        && !wait.timed_out
                    {
                        wait.timed_out = true;
                        expired.push(tid);
                    }
                }
            }
        }
        waits
            .deadlines
            .iter()
            .filter_map(|(clock_id, deadlines)| {
                deadlines
                    .first_key_value()
                    .map(|(deadline_us, _)| (*clock_id, *deadline_us))
            })
            .collect::<Vec<_>>()
    };

    for tid in expired {
        crate::task::wakeup_task(tid);
    }
    for (clock_id, deadline_us) in next_deadlines {
        if let Ok(now_us) = nanosleep_clock_us(clock_id) {
            crate::timer::request_task_timer_after_us(deadline_us.saturating_sub(now_us));
        }
    }
}

pub fn sys_clock_getres(clock_id: usize, res: *mut TimeSpec) -> SysResult<usize> {
    if !is_readable_clock(clock_id) {
        return Err(Errno::EINVAL);
    }
    if !res.is_null() {
        let nsec = if matches!(clock_id, CLOCK_REALTIME_COARSE | CLOCK_MONOTONIC_COARSE) {
            CLOCK_COARSE_RESOLUTION_NS
        } else {
            CLOCK_FINE_RESOLUTION_NS
        };
        let time_spec = TimeSpec { sec: 0, nsec };
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
            .saturating_sub(timer.clock_time_ms().unwrap_or(usize::MAX))
    }
}

impl PosixTimer {
    fn clock_time_ms(&self) -> SysResult<usize> {
        match self.clock_id {
            CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => self
                .cpu_clock
                .as_ref()
                .map(|clock| clock.now_us() / 1000)
                .ok_or(Errno::EINVAL),
            _ => clock_time_ms(self.clock_id),
        }
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
    if !is_posix_timer_clock(clock_id) {
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
    let cpu_clock = match clock_id {
        CLOCK_PROCESS_CPUTIME_ID => Some(task.process_cpu_clock()),
        CLOCK_THREAD_CPUTIME_ID => Some(task.thread_cpu_clock()),
        _ => None,
    };
    let owner = TASK_MANAGER
        .get(task.tgid())
        .unwrap_or_else(|| Arc::clone(&task));
    let id = NEXT_POSIX_TIMER_ID.fetch_add(1, Ordering::Relaxed) as i32;
    let timer = PosixTimer {
        owner_tgid: task.tgid(),
        owner: Arc::downgrade(&owner),
        clock_id,
        cpu_clock,
        signo,
        deadline_ms: 0,
        interval_ms: 0,
    };

    // timerid is the only handle by which userspace can subsequently manage
    // this object. Do not publish the timer until that handle is visible.
    copy_to_user(timerid, &id as *const i32, 1)?;
    POSIX_TIMERS.lock().insert(id as usize, timer);
    Ok(0)
}

/// Remove all POSIX timers owned by a process that is completing group exit.
///
/// Timers currently use the numeric tgid as their owner identity, so leaving
/// one behind would also allow it to target an unrelated task after PID reuse.
pub fn remove_posix_timers_for_owner(owner_tgid: usize) {
    let removed = {
        let mut timers = POSIX_TIMERS.lock();
        let before = timers.len();
        timers.retain(|_, timer| timer.owner_tgid != owner_tgid);
        before - timers.len()
    };
    #[cfg(feature = "debug_traces")]
    {
        if TIMER_LIFECYCLE_TRACE {
            println!("[timer-lifecycle] owner={} removed={}", owner_tgid, removed);
        }
    }
    #[cfg(not(feature = "debug_traces"))]
    let _ = removed;
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
    let prepared_timer = {
        let timers = POSIX_TIMERS.lock();
        let timer = timers.get(&timerid).ok_or(Errno::EINVAL)?;
        if timer.owner_tgid != task.tgid() {
            return Err(Errno::EINVAL);
        }
        timer.clone()
    };
    let old = posix_timer_snapshot(&prepared_timer);
    let now_ms = prepared_timer.clock_time_ms()?;
    let deadline_ms = if value_ms == 0 {
        0
    } else if flags & TIMER_ABSTIME != 0 {
        value_ms.max(now_ms)
    } else {
        now_ms.saturating_add(value_ms)
    };

    if !old_value.is_null() {
        copy_to_user(old_value, &old as *const ITimerSpec, 1)?;
    }

    let mut timers = POSIX_TIMERS.lock();
    let timer = timers.get_mut(&timerid).ok_or(Errno::EINVAL)?;
    if timer.owner_tgid != task.tgid() || timer.clock_id != prepared_timer.clock_id {
        return Err(Errno::EINVAL);
    }
    timer.deadline_ms = deadline_ms;
    timer.interval_ms = interval_ms;
    Ok(0)
}

pub fn check_posix_timers() {
    let mut expired = Vec::new();
    {
        let mut timers = POSIX_TIMERS.lock();
        for timer in timers.values_mut() {
            let Ok(now_ms) = timer.clock_time_ms() else {
                continue;
            };
            if timer.deadline_ms == 0 || now_ms < timer.deadline_ms {
                continue;
            }
            expired.push((timer.owner.clone(), timer.signo));
            timer.deadline_ms = if timer.interval_ms == 0 {
                0
            } else {
                now_ms.saturating_add(timer.interval_ms)
            };
        }
    }

    for (owner, signo) in expired {
        let sig = Sig::from(signo);
        if sig.is_valid() {
            if let Some(task) = owner.upgrade().filter(|task| !task.is_exited()) {
                let siginfo = SigInfo::new(sig.raw(), SigInfo::KERNEL, SiField::None);
                task.receive_siginfo(siginfo, false);
            }
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
    let time_us = req_time.checked_duration_us().ok_or(Errno::EINVAL)?;
    let start_us = get_timeout_us();
    let deadline_us = start_us.checked_add(time_us).ok_or(Errno::EINVAL)?;
    sleep_until_us(
        NANOSLEEP_TIMEOUT_CLOCK,
        deadline_us,
        Some((start_us, time_us, rem)),
    )
}

pub fn sys_clock_nanosleep(
    clock_id: usize,
    flags: usize,
    req: *const TimeSpec,
    rem: *mut TimeSpec,
) -> SysResult<usize> {
    const TIMER_ABSTIME: usize = 1;

    if flags & !TIMER_ABSTIME != 0 {
        return Err(Errno::EINVAL);
    }
    if clock_id == CLOCK_THREAD_CPUTIME_ID {
        return Err(Errno::EOPNOTSUPP);
    }
    if !is_nanosleep_clock(clock_id) {
        return Err(Errno::EINVAL);
    }
    if flags & TIMER_ABSTIME == 0 {
        return sys_nanosleep(req, rem);
    }

    let mut req_time = TimeSpec::default();
    copy_from_user(&mut req_time as *mut TimeSpec, req, 1)?;
    let deadline_us = req_time.checked_duration_us().ok_or(Errno::EINVAL)?;
    sleep_until_us(clock_id, deadline_us, None)
}

fn write_remaining_time(start_us: usize, total_us: usize, rem: *mut TimeSpec) -> SysResult<()> {
    if rem.is_null() {
        return Ok(());
    }
    let elapsed_us = get_timeout_us().saturating_sub(start_us).min(total_us);
    let remain = timespec_from_us(total_us - elapsed_us);
    copy_to_user(rem, &remain as *const TimeSpec, 1)?;
    Ok(())
}

fn sleep_until_us(
    clock_id: usize,
    deadline_us: usize,
    relative_rem: Option<(usize, usize, *mut TimeSpec)>,
) -> SysResult<usize> {
    let task = current_task().expect("no current task");

    loop {
        if nanosleep_clock_us(clock_id)? >= deadline_us {
            return Ok(0);
        }
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            if let Some((start_us, total_us, rem)) = relative_rem {
                write_remaining_time(start_us, total_us, rem)?;
            }
            return Err(Errno::EINTR);
        }

        task.set_interruptible(true);
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            task.set_interruptible(false);
            if let Some((start_us, total_us, rem)) = relative_rem {
                write_remaining_time(start_us, total_us, rem)?;
            }
            return Err(Errno::EINTR);
        }
        if !prepare_current_task_blocked() {
            crate::perf::signal_time_yield(1);
            yield_current_task();
            task.set_interruptible(false);
            continue;
        }
        register_nanosleep_wait(task.tid(), clock_id, deadline_us);
        switch_to_next_task();
        task.set_interruptible(false);

        if task.is_interrupted() || task.check_signal_interrupt() {
            task.clear_interrupted();
            finish_nanosleep_wait(task.tid());
            if let Some((start_us, total_us, rem)) = relative_rem {
                write_remaining_time(start_us, total_us, rem)?;
            }
            return Err(Errno::EINTR);
        }
        if finish_nanosleep_wait(task.tid()) {
            return Ok(0);
        }
    }
}
