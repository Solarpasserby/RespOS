use super::time::{ITimerSpec, clock_time_ms};
use super::{Errno, SysResult};
use crate::fs::vfs::InodeType;
use crate::fs::{
    FdEntry, FileOp, KStat, OpenFlags, POLL_READ, POLL_WRITE, PollEvents, PollWaiters, SpecialFd,
};
use crate::mm::{check_user_readable, copy_cstr_from_user, copy_from_user, copy_to_user};
use crate::mutex::SpinLock;
use crate::task::{
    TASK_MANAGER, current_task, prepare_current_task_blocked, remove_task, switch_to_next_task,
    yield_current_task,
};
use crate::timer::TimeSpec;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use lazy_static::lazy_static;

const O_NONBLOCK: usize = OpenFlags::O_NONBLOCK.bits() as usize;
const O_CLOEXEC: usize = OpenFlags::O_CLOEXEC.bits() as usize;
const EFD_SEMAPHORE: usize = 1;
const TFD_TIMER_ABSTIME: usize = 1;
const TFD_TIMER_CANCEL_ON_SET: usize = 1 << 1;
const TFD_SETTIME_FLAGS: usize = TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET;

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

#[derive(Clone, Copy, Default)]
struct TimerFdState {
    interval_ms: usize,
    deadline_ms: usize,
    consumed: u64,
}

pub struct EventFd {
    flags: OpenFlags,
    semaphore: bool,
    counter: SpinLock<u64>,
    poll_waiters: PollWaiters,
}

impl EventFd {
    fn new(initval: usize, flags: OpenFlags, semaphore: bool) -> Self {
        Self {
            flags,
            semaphore,
            counter: SpinLock::new(initval as u64),
            poll_waiters: PollWaiters::new(),
        }
    }
}

fn wait_for_file_event(
    waiters: &PollWaiters,
    events: PollEvents,
    ready: impl Fn() -> bool,
) -> SysResult {
    let task = current_task().expect("[kernel] current task is None.");
    task.set_interruptible(true);
    waiters.register(task.tid(), events);

    if ready() {
        waiters.unregister(task.tid());
        task.set_interruptible(false);
        return Ok(());
    }
    if task.check_signal_interrupt() || task.is_interrupted() {
        task.clear_interrupted();
        waiters.unregister(task.tid());
        task.set_interruptible(false);
        return Err(Errno::EINTR);
    }

    if prepare_current_task_blocked() {
        if task.is_ready() {
            remove_task(task.tid());
            task.set_running();
        } else {
            switch_to_next_task();
        }
    } else {
        yield_current_task();
    }

    waiters.unregister(task.tid());
    task.set_interruptible(false);
    if task.check_signal_interrupt() || task.is_interrupted() {
        task.clear_interrupted();
        return Err(Errno::EINTR);
    }
    Ok(())
}

impl FileOp for EventFd {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        if buf.len() < core::mem::size_of::<u64>() {
            return Err(Errno::EINVAL);
        }
        loop {
            let mut counter = self.counter.lock();
            if *counter != 0 {
                let value = if self.semaphore { 1 } else { *counter };
                *counter -= value;
                buf[..8].copy_from_slice(&value.to_ne_bytes());
                drop(counter);
                self.poll_waiters.notify(POLL_WRITE);
                return Ok(8);
            }
            drop(counter);
            if self.flags.contains(OpenFlags::O_NONBLOCK) {
                return Err(Errno::EAGAIN);
            }
            wait_for_file_event(&self.poll_waiters, POLL_READ, || self.read_ready())?;
        }
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        if buf.len() < core::mem::size_of::<u64>() {
            return Err(Errno::EINVAL);
        }
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&buf[..8]);
        let value = u64::from_ne_bytes(raw);
        if value == u64::MAX {
            return Err(Errno::EINVAL);
        }
        loop {
            let mut counter = self.counter.lock();
            if value <= (u64::MAX - 1).saturating_sub(*counter) {
                *counter += value;
                drop(counter);
                self.poll_waiters.notify(POLL_READ);
                return Ok(8);
            }
            drop(counter);
            if self.flags.contains(OpenFlags::O_NONBLOCK) {
                return Err(Errno::EAGAIN);
            }
            wait_for_file_event(&self.poll_waiters, POLL_WRITE, || {
                value <= (u64::MAX - 1).saturating_sub(*self.counter.lock())
            })?;
        }
    }

    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn get_flags(&self) -> OpenFlags {
        self.flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Unknown))
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read_ready(&self) -> bool {
        *self.counter.lock() != 0
    }

    fn write_ready(&self) -> bool {
        *self.counter.lock() < u64::MAX - 1
    }

    fn register_poll_waiter(&self, tid: usize, events: PollEvents) -> bool {
        self.poll_waiters.register(tid, events);
        true
    }

    fn unregister_poll_waiter(&self, tid: usize) {
        self.poll_waiters.unregister(tid);
    }
}

pub struct TimerFd {
    clockid: usize,
    flags: OpenFlags,
    state: SpinLock<TimerFdState>,
    poll_waiters: PollWaiters,
}

impl TimerFd {
    fn new(clockid: usize, flags: OpenFlags) -> Self {
        Self {
            clockid,
            flags,
            state: SpinLock::new(TimerFdState::default()),
            poll_waiters: PollWaiters::new(),
        }
    }

    fn expirations_locked(state: &TimerFdState, now_ms: usize) -> u64 {
        if state.deadline_ms == 0 || now_ms < state.deadline_ms {
            return 0;
        }
        if state.interval_ms == 0 {
            return 1;
        }
        1 + ((now_ms - state.deadline_ms) / state.interval_ms) as u64
    }

    fn pending(&self) -> u64 {
        let state = self.state.lock();
        Self::expirations_locked(&state, clock_time_ms(self.clockid).unwrap_or(0))
            .saturating_sub(state.consumed)
    }

    fn current_spec(&self) -> ITimerSpec {
        let now_ms = clock_time_ms(self.clockid).unwrap_or(0);
        let state = self.state.lock();
        let remaining_ms = if state.deadline_ms == 0 {
            0
        } else if now_ms < state.deadline_ms {
            state.deadline_ms - now_ms
        } else if state.interval_ms == 0 {
            0
        } else {
            let elapsed = now_ms - state.deadline_ms;
            let rem = state.interval_ms - (elapsed % state.interval_ms);
            if rem == state.interval_ms { 0 } else { rem }
        };
        ITimerSpec {
            interval: ms_to_timespec(state.interval_ms),
            value: ms_to_timespec(remaining_ms),
        }
    }
}

impl FileOp for TimerFd {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        if buf.len() < core::mem::size_of::<u64>() {
            return Err(Errno::EINVAL);
        }
        loop {
            let mut state = self.state.lock();
            let expired = Self::expirations_locked(&state, clock_time_ms(self.clockid)?);
            let pending = expired.saturating_sub(state.consumed);
            if pending != 0 {
                state.consumed = expired;
                buf[..8].copy_from_slice(&pending.to_ne_bytes());
                return Ok(8);
            }
            drop(state);
            if self.flags.contains(OpenFlags::O_NONBLOCK) {
                return Err(Errno::EAGAIN);
            }
            wait_for_file_event(&self.poll_waiters, POLL_READ, || self.read_ready())?;
        }
    }

    fn write<'a>(&'a self, _buf: &'a [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn get_flags(&self) -> OpenFlags {
        self.flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Unknown))
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn read_ready(&self) -> bool {
        self.pending() > 0
    }

    fn register_poll_waiter(&self, tid: usize, events: PollEvents) -> bool {
        self.poll_waiters.register(tid, events);
        true
    }

    fn unregister_poll_waiter(&self, tid: usize) {
        self.poll_waiters.unregister(tid);
    }
}

lazy_static! {
    static ref TIMERFDS: SpinLock<Vec<Weak<TimerFd>>> = SpinLock::new(Vec::new());
}

pub fn check_timerfd_expirations() {
    let timerfds = {
        let mut registry = TIMERFDS.lock();
        let mut live = Vec::new();
        registry.retain(|timerfd| {
            if let Some(timerfd) = timerfd.upgrade() {
                live.push(timerfd);
                true
            } else {
                false
            }
        });
        live
    };
    for timerfd in timerfds {
        if timerfd.read_ready() {
            timerfd.poll_waiters.notify(POLL_READ);
        }
    }
}

fn is_timerfd_clock(clockid: usize) -> bool {
    const CLOCK_REALTIME: usize = 0;
    const CLOCK_MONOTONIC: usize = 1;
    const CLOCK_BOOTTIME: usize = 7;
    const CLOCK_REALTIME_ALARM: usize = 8;
    const CLOCK_BOOTTIME_ALARM: usize = 9;

    matches!(
        clockid,
        CLOCK_REALTIME
            | CLOCK_MONOTONIC
            | CLOCK_BOOTTIME
            | CLOCK_REALTIME_ALARM
            | CLOCK_BOOTTIME_ALARM
    )
}

fn ms_to_timespec(ms: usize) -> TimeSpec {
    TimeSpec {
        sec: (ms / 1000) as isize,
        nsec: ((ms % 1000) * 1_000_000) as isize,
    }
}

fn absolute_timespec_ms(ts: TimeSpec) -> SysResult<usize> {
    if !ts.is_valid_duration() {
        return Err(Errno::EINVAL);
    }
    (ts.sec as usize)
        .checked_mul(1000)
        .and_then(|ms| ms.checked_add((ts.nsec as usize) / 1_000_000))
        .ok_or(Errno::EINVAL)
}

fn timerfd_ref(fd: usize) -> SysResult<Arc<dyn FileOp>> {
    let task = current_task().expect("[kernel] current task is None.");
    let entry = task.get_fd_entry(fd)?;
    if entry.file.as_any().downcast_ref::<TimerFd>().is_none() {
        return Err(Errno::EINVAL);
    }
    Ok(entry.file)
}

pub fn sys_eventfd2(initval: usize, flags: usize) -> SysResult<usize> {
    let fd_flags = flags_from_o_flags(flags, EFD_SEMAPHORE | O_NONBLOCK | O_CLOEXEC)?;
    let task = current_task().expect("[kernel] current task is None.");
    let file = Arc::new(EventFd::new(initval, fd_flags, flags & EFD_SEMAPHORE != 0));
    task.alloc_fd(FdEntry::new(file, fd_flags))
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

pub fn sys_timerfd_create(clockid: usize, flags: usize) -> SysResult<usize> {
    if !is_timerfd_clock(clockid) {
        return Err(Errno::EINVAL);
    }
    let flags = flags_from_o_flags(flags, O_NONBLOCK | O_CLOEXEC)?;
    let task = current_task().expect("[kernel] current task is None.");
    let timerfd = Arc::new(TimerFd::new(clockid, flags));
    TIMERFDS.lock().push(Arc::downgrade(&timerfd));
    task.alloc_fd(FdEntry::new(timerfd, flags))
}

pub fn sys_timerfd_gettime(fd: usize, curr_value: *mut ITimerSpec) -> SysResult<usize> {
    let file = timerfd_ref(fd)?;
    let timerfd = file.as_any().downcast_ref::<TimerFd>().unwrap();
    let current = timerfd.current_spec();
    copy_to_user(curr_value, &current as *const ITimerSpec, 1)?;
    Ok(0)
}

pub fn sys_timerfd_settime(
    fd: usize,
    flags: usize,
    new_value: *const ITimerSpec,
    old_value: *mut ITimerSpec,
) -> SysResult<usize> {
    let file = timerfd_ref(fd)?;
    if flags & !TFD_SETTIME_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    let timerfd = file.as_any().downcast_ref::<TimerFd>().unwrap();
    let old = timerfd.current_spec();
    let mut new_timer = ITimerSpec::default();
    copy_from_user(&mut new_timer as *mut ITimerSpec, new_value, 1)?;
    if !new_timer.value.is_valid_duration() || !new_timer.interval.is_valid_duration() {
        return Err(Errno::EINVAL);
    }
    if !old_value.is_null() {
        copy_to_user(old_value, &old as *const ITimerSpec, 1)?;
    }

    let value_ms = if flags & TFD_TIMER_ABSTIME != 0 {
        absolute_timespec_ms(new_timer.value)?
    } else {
        new_timer.value.checked_duration_ms().ok_or(Errno::EINVAL)?
    };
    let interval_ms = new_timer
        .interval
        .checked_duration_ms()
        .ok_or(Errno::EINVAL)?;
    let now_ms = clock_time_ms(timerfd.clockid)?;
    let deadline_ms = if value_ms == 0 {
        0
    } else if flags & TFD_TIMER_ABSTIME != 0 {
        value_ms
    } else {
        now_ms.saturating_add(value_ms)
    };

    let mut state = timerfd.state.lock();
    *state = TimerFdState {
        interval_ms,
        deadline_ms,
        consumed: 0,
    };
    drop(state);
    if timerfd.read_ready() {
        timerfd.poll_waiters.notify(POLL_READ);
    }
    Ok(0)
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
    const MEMFD_NAME_MAX: usize = 249;
    if flags & !MFD_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    let name = copy_cstr_from_user(name)?;
    if name.len() > MEMFD_NAME_MAX {
        return Err(Errno::EINVAL);
    }
    let task = current_task().expect("[kernel] current task is None.");
    let fd_flags = fd_flags(false, flags & MFD_CLOEXEC != 0);
    let file = Arc::new(SpecialFd::new_memfd(
        fd_flags,
        flags & MFD_ALLOW_SEALING != 0,
    ));
    task.alloc_fd(FdEntry::new(file, fd_flags))
}

pub fn sys_memfd_secret(flags: usize) -> SysResult<usize> {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }
    alloc_special_fd_with_type(OpenFlags::O_RDWR, InodeType::Regular)
}
