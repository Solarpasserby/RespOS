use super::time::{ITimerSpec, clock_time_ms};
use super::{Errno, SysResult};
use crate::fs::vfs::InodeType;
use crate::fs::{
    FdEntry, FileOp, KStat, OpenFlags, POLL_READ, POLL_WRITE, PollEvents, PollWaiters, SpecialFd,
};
use crate::mm::{copy_cstr_from_user, copy_from_user, copy_to_user};
use crate::mutex::SpinLock;
use crate::signal::sig_struct::{Sig, SigSet};
use crate::task::{
    current_task, prepare_current_task_blocked, remove_task, switch_to_next_task,
    yield_current_task,
};
use crate::timer::{TimeSpec, get_timeout_us};
use alloc::collections::BTreeMap;
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
const MFD_ALLOWED_FLAGS: usize = MFD_CLOEXEC | MFD_ALLOW_SEALING;

pub struct EpollFd {
    flags: OpenFlags,
    interests: SpinLock<BTreeMap<(usize, usize), EpollInterest>>,
}

struct EpollInterest {
    events: u32,
    data: u64,
    file: Arc<dyn FileOp>,
    last_ready: u32,
    disabled: bool,
    generation: usize,
}

struct EpollReady {
    key: (usize, usize),
    events: u32,
    data: u64,
    observed_ready: u32,
    generation: usize,
}

impl EpollFd {
    fn new(flags: OpenFlags) -> Self {
        Self {
            flags,
            interests: SpinLock::new(BTreeMap::new()),
        }
    }

    fn ctl(
        &self,
        op: usize,
        fd: usize,
        event: Option<(u32, u64)>,
        file: Arc<dyn FileOp>,
    ) -> SysResult<usize> {
        const EPOLL_CTL_ADD: usize = 1;
        const EPOLL_CTL_DEL: usize = 2;
        const EPOLL_CTL_MOD: usize = 3;

        let identity = Arc::as_ptr(&file) as *const () as usize;
        let key = (fd, identity);
        let mut interests = self.interests.lock();
        match op {
            EPOLL_CTL_ADD => {
                if interests.contains_key(&key) {
                    return Err(Errno::EEXIST);
                }
                let (events, data) = event.ok_or(Errno::EFAULT)?;
                interests.insert(
                    key,
                    EpollInterest {
                        events,
                        data,
                        file,
                        last_ready: 0,
                        disabled: false,
                        generation: 0,
                    },
                );
            }
            EPOLL_CTL_DEL => {
                if interests.remove(&key).is_none() {
                    return Err(Errno::ENOENT);
                }
            }
            EPOLL_CTL_MOD => {
                let interest = interests.get_mut(&key).ok_or(Errno::ENOENT)?;
                let (events, data) = event.ok_or(Errno::EFAULT)?;
                interest.events = events;
                interest.data = data;
                interest.last_ready = 0;
                interest.disabled = false;
                interest.generation = interest.generation.wrapping_add(1);
            }
            _ => return Err(Errno::EINVAL),
        }
        Ok(0)
    }

    fn scan_ready(&self, maxevents: usize, out: &mut Vec<EpollReady>) -> usize {
        const EPOLLIN: u32 = 0x001;
        const EPOLLOUT: u32 = 0x004;
        const EPOLLERR: u32 = 0x008;
        const EPOLLHUP: u32 = 0x010;
        const EPOLLRDHUP: u32 = 0x2000;
        const EPOLLET: u32 = 1 << 31;

        let task = current_task().expect("[kernel] current task is None.");
        let open_files: Vec<_> = task
            .open_fds()
            .into_iter()
            .filter_map(|fd| task.get_fd_entry(fd).ok().map(|entry| entry.file))
            .collect();
        let mut interests = self.interests.lock();
        interests.retain(|_, interest| {
            open_files
                .iter()
                .any(|file| Arc::ptr_eq(file, &interest.file))
        });
        for (key, interest) in interests.iter_mut() {
            if out.len() >= maxevents {
                break;
            }
            if interest.disabled {
                continue;
            }

            let mut ready = 0;
            if interest.events & EPOLLIN != 0
                && interest.file.readable()
                && interest.file.read_ready()
            {
                ready |= EPOLLIN;
            }
            if interest.events & EPOLLOUT != 0
                && interest.file.writable()
                && interest.file.write_ready()
            {
                ready |= EPOLLOUT;
            }
            if interest.file.poll_error() {
                ready |= EPOLLERR;
            }
            if interest.file.poll_hup() {
                ready |= EPOLLHUP;
            }
            if interest.events & EPOLLRDHUP != 0 && interest.file.poll_rdhup() {
                ready |= EPOLLRDHUP;
            }

            let report = if interest.events & EPOLLET != 0 {
                let newly_ready = ready & !interest.last_ready;
                // ready -> not-ready 不消费用户可见事件，可以立即提交；真正
                // 的 ready 事件要等 copyout 成功后再更新 last_ready。
                if ready == 0 {
                    interest.last_ready = 0;
                }
                newly_ready
            } else {
                ready
            };
            if report == 0 {
                continue;
            }

            out.push(EpollReady {
                key: *key,
                events: report & (interest.events | EPOLLERR | EPOLLHUP),
                data: interest.data,
                observed_ready: ready,
                generation: interest.generation,
            });
        }
        out.len()
    }

    fn commit_ready(&self, ready: &[EpollReady]) {
        const EPOLLET: u32 = 1 << 31;
        const EPOLLONESHOT: u32 = 1 << 30;

        let mut interests = self.interests.lock();
        for event in ready {
            let Some(interest) = interests.get_mut(&event.key) else {
                continue;
            };
            if interest.generation != event.generation {
                continue;
            }
            if interest.events & EPOLLET != 0 {
                interest.last_ready = event.observed_ready;
            }
            if interest.events & EPOLLONESHOT != 0 {
                interest.disabled = true;
            }
        }
    }

    fn register_waiters(&self, tid: usize) -> Vec<Arc<dyn FileOp>> {
        const EPOLLIN: u32 = 0x001;
        const EPOLLOUT: u32 = 0x004;

        let interests = self.interests.lock();
        let mut files = Vec::new();
        for interest in interests.values() {
            if interest.disabled {
                continue;
            }
            let mut events = crate::fs::POLL_HUP;
            if interest.events & EPOLLIN != 0 {
                events |= POLL_READ;
            }
            if interest.events & EPOLLOUT != 0 {
                events |= POLL_WRITE;
            }
            if events != 0 && interest.file.register_poll_waiter(tid, events) {
                files.push(interest.file.clone());
            }
        }
        files
    }
}

impl FileOp for EpollFd {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, _buf: &'a mut [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
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
        false
    }

    fn writable(&self) -> bool {
        false
    }
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
    flags: SpinLock<OpenFlags>,
    semaphore: bool,
    counter: SpinLock<u64>,
    poll_waiters: PollWaiters,
}

impl EventFd {
    fn new(initval: usize, flags: OpenFlags, semaphore: bool) -> Self {
        Self {
            flags: SpinLock::new(flags),
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
        crate::perf::special_fd_yield(1);
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
            if self.flags.lock().contains(OpenFlags::O_NONBLOCK) {
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
            if self.flags.lock().contains(OpenFlags::O_NONBLOCK) {
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
        *self.flags.lock()
    }

    fn set_status_flags(&self, flags: OpenFlags) -> SysResult {
        let mut current = self.flags.lock();
        current.remove(OpenFlags::O_NONBLOCK);
        *current |= flags & OpenFlags::O_NONBLOCK;
        Ok(())
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
    flags: SpinLock<OpenFlags>,
    state: SpinLock<TimerFdState>,
    poll_waiters: PollWaiters,
}

impl TimerFd {
    fn new(clockid: usize, flags: OpenFlags) -> Self {
        Self {
            clockid,
            flags: SpinLock::new(flags),
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
            if self.flags.lock().contains(OpenFlags::O_NONBLOCK) {
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
        *self.flags.lock()
    }

    fn set_status_flags(&self, flags: OpenFlags) -> SysResult {
        let mut current = self.flags.lock();
        current.remove(OpenFlags::O_NONBLOCK);
        *current |= flags & OpenFlags::O_NONBLOCK;
        Ok(())
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

    matches!(clockid, CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_BOOTTIME)
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
    let task = current_task().expect("[kernel] current task is None.");
    task.alloc_fd(FdEntry::new(Arc::new(EpollFd::new(flags)), flags))
}

pub fn sys_epoll_ctl(epfd: usize, op: usize, fd: usize, event: *const u8) -> SysResult<usize> {
    const EPOLL_CTL_ADD: usize = 1;
    const EPOLL_CTL_DEL: usize = 2;
    const EPOLL_CTL_MOD: usize = 3;

    let task = current_task().expect("[kernel] current task is None.");
    let epoll_entry = task.get_fd_entry(epfd)?;
    let epoll = epoll_entry
        .file
        .as_any()
        .downcast_ref::<EpollFd>()
        .ok_or(Errno::EINVAL)?;
    let target = task.get_fd_entry(fd)?;
    if epfd == fd {
        return Err(Errno::EINVAL);
    }
    if !matches!(op, EPOLL_CTL_ADD | EPOLL_CTL_DEL | EPOLL_CTL_MOD) {
        return Err(Errno::EINVAL);
    }
    if matches!(
        target.file.get_stat()?.ty,
        InodeType::Regular | InodeType::Directory | InodeType::BlockDevice
    ) {
        return Err(Errno::EPERM);
    }

    let event = if op == EPOLL_CTL_DEL {
        None
    } else {
        // Tokio/mio registers every source with EPOLLRDHUP. The bit is valid
        // for stream descriptors; TCP reports it through FileOp::poll_rdhup,
        // while stream backends without a distinct RDHUP state may still
        // expose EOF through ordinary EPOLLIN.
        const EPOLLPRI: u32 = 0x002;
        const EPOLLRDHUP: u32 = 0x2000;
        const EPOLL_SUPPORTED_EVENTS: u32 =
            0x001 | EPOLLPRI | 0x004 | 0x008 | 0x010 | EPOLLRDHUP | (1 << 31) | (1 << 30);
        let mut raw = [0u8; 12];
        copy_from_user(raw.as_mut_ptr(), event, raw.len())?;
        let events = u32::from_ne_bytes(raw[..4].try_into().unwrap());
        if events & !EPOLL_SUPPORTED_EVENTS != 0 {
            return Err(Errno::EOPNOTSUPP);
        }
        Some((events, u64::from_ne_bytes(raw[4..].try_into().unwrap())))
    };
    epoll.ctl(op, fd, event, target.file)
}

fn write_epoll_events(events: *mut u8, ready: &[EpollReady]) -> SysResult<usize> {
    for (idx, event) in ready.iter().enumerate() {
        let mut raw = [0u8; 12];
        raw[..4].copy_from_slice(&event.events.to_ne_bytes());
        raw[4..].copy_from_slice(&event.data.to_ne_bytes());
        let dst = unsafe { events.add(idx * raw.len()) };
        copy_to_user(dst, raw.as_ptr(), raw.len())?;
    }
    Ok(ready.len())
}

fn epoll_sigmask(sigmask: *const u8, sigsetsize: usize) -> SysResult<Option<SigSet>> {
    if sigmask.is_null() {
        return Ok(None);
    }
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return Err(Errno::EINVAL);
    }
    let mut mask = SigSet::empty();
    copy_from_user(&mut mask as *mut SigSet, sigmask.cast(), 1)?;
    mask.remove_signal(Sig::SIGKILL);
    mask.remove_signal(Sig::SIGSTOP);
    Ok(Some(mask))
}

pub fn sys_epoll_pwait(
    epfd: usize,
    events: *mut u8,
    maxevents: usize,
    timeout_ms: isize,
    sigmask: *const u8,
    sigsetsize: usize,
) -> SysResult<usize> {
    const EPOLL_MAX_EVENTS: usize = 4096;

    if maxevents == 0 || maxevents > EPOLL_MAX_EVENTS {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("[kernel] current task is None.");
    let epoll_entry = task.get_fd_entry(epfd)?;
    let epoll = epoll_entry
        .file
        .as_any()
        .downcast_ref::<EpollFd>()
        .ok_or(Errno::EINVAL)?;
    let new_mask = epoll_sigmask(sigmask, sigsetsize)?;
    let original_mask = new_mask.map(|mask| {
        task.op_sig_pending_mut(|pending| {
            let original = pending.mask;
            pending.mask = mask;
            original
        })
    });

    let result = (|| {
        let deadline_us = if timeout_ms < 0 {
            None
        } else {
            Some(get_timeout_us().saturating_add((timeout_ms as usize).saturating_mul(1000)))
        };

        let mut ready = Vec::new();
        loop {
            ready.clear();
            if epoll.scan_ready(maxevents, &mut ready) > 0 {
                let copied = write_epoll_events(events, &ready)?;
                epoll.commit_ready(&ready);
                return Ok(copied);
            }

            if timeout_ms == 0 || deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline) {
                return Ok(0);
            }

            task.set_interruptible(true);
            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }

            let registered = epoll.register_waiters(task.tid());
            if registered.is_empty() {
                task.set_interruptible(false);
                crate::perf::special_fd_yield(1);
                yield_current_task();
                continue;
            }

            if let Some(deadline_us) = deadline_us {
                super::register_task_timeout_us(task.tid(), deadline_us);
            }

            if prepare_current_task_blocked() {
                ready.clear();
                let became_ready = epoll.scan_ready(maxevents, &mut ready) > 0;
                let timed_out = deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline);
                let interrupted = task.check_signal_interrupt() || task.is_interrupted();
                if became_ready || timed_out || interrupted || task.is_ready() {
                    remove_task(task.tid());
                    task.set_running();
                } else {
                    switch_to_next_task();
                }
            } else {
                crate::perf::special_fd_yield(1);
                yield_current_task();
            }

            let timed_out = if deadline_us.is_some() {
                super::finish_task_timeout(task.tid())
            } else {
                false
            };
            for file in registered {
                file.unregister_poll_waiter(task.tid());
            }
            task.set_interruptible(false);

            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                return Err(Errno::EINTR);
            }
            if timed_out || deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline) {
                return Ok(0);
            }
        }
    })();

    task.set_interruptible(false);
    if let Some(original_mask) = original_mask {
        task.op_sig_pending_mut(|pending| pending.mask = original_mask);
    }
    result
}

pub fn sys_inotify_init1(_flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_signalfd4(
    _fd: isize,
    _mask: *const u8,
    _sizemask: usize,
    _flags: usize,
) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_timerfd_create(clockid: usize, flags: usize) -> SysResult<usize> {
    const CLOCK_REALTIME_ALARM: usize = 8;
    const CLOCK_BOOTTIME_ALARM: usize = 9;
    if matches!(clockid, CLOCK_REALTIME_ALARM | CLOCK_BOOTTIME_ALARM) {
        return Err(Errno::EOPNOTSUPP);
    }
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
    if flags & TFD_TIMER_CANCEL_ON_SET != 0 {
        return Err(Errno::EOPNOTSUPP);
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

pub fn sys_pidfd_open(_pid: usize, _flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_fanotify_init(_flags: usize, _event_f_flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_userfaultfd(_flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_perf_event_open(
    _attr: *const u8,
    _pid: isize,
    _cpu: isize,
    _group_fd: isize,
    _flags: usize,
) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_io_uring_setup(_entries: usize, _params: *const u8) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_bpf(_cmd: usize, _attr: *const u8, _size: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_fsopen(_fs_name: *const u8, _flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_fspick(_dfd: isize, _path: *const u8, _flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_open_tree(_dfd: isize, _path: *const u8, _flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}

pub fn sys_memfd_create(name: *const u8, flags: usize) -> SysResult<usize> {
    const MEMFD_NAME_MAX: usize = 249;
    const MFD_RECOGNIZED_FLAGS: usize = MFD_ALLOWED_FLAGS | MFD_HUGETLB | MFD_HUGE_MASK;
    if flags & !MFD_RECOGNIZED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    if flags & MFD_HUGE_MASK != 0 && flags & MFD_HUGETLB == 0 {
        return Err(Errno::EINVAL);
    }
    if flags & MFD_HUGETLB != 0 {
        return Err(Errno::EOPNOTSUPP);
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

pub fn sys_memfd_secret(_flags: usize) -> SysResult<usize> {
    Err(Errno::ENOSYS)
}
