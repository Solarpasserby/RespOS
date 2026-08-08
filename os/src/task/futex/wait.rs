// os/src/task/futex/wait.rs

use super::queue::{FUTEX_QUEUES, FutexKey, FutexQ, futex_hash_idx};
use crate::config::PAGE_SIZE;
use crate::mm::{VirtAddr, check_user_readable, copy_from_user, read_user_u32_nofault};
use crate::mutex::SpinNoIrqLock;
use crate::syscall::{Errno, SysResult};
use crate::task::scheduler::{
    prepare_current_task_blocked, remove_task, switch_to_next_task, wakeup_task,
};
use crate::task::{current_task, futex::FUTEX_BITSET_MATCH_ANY, yield_current_task};
use crate::timer::{TimeSpec, get_time_us, get_timeout_us};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use lazy_static::lazy_static;

const FUTEX_TRACE: bool = false;
const FUTEX_EXIT_TRACE: bool = option_env!("TASK_A_FUTEX_EXIT_TRACE").is_some();
const FUTEX_CMP_REQUEUE_TEST_YIELD: bool =
    option_env!("TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD").is_some();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaitCompletion {
    Pending,
    Woken,
    TimedOut,
    Interrupted,
}

#[derive(Clone, Copy)]
enum FutexDeadline {
    UserClock(usize),
    TimeoutClock(usize),
}

impl FutexDeadline {
    fn micros(self) -> usize {
        match self {
            FutexDeadline::UserClock(deadline_us) | FutexDeadline::TimeoutClock(deadline_us) => {
                deadline_us
            }
        }
    }

    fn expired(self) -> bool {
        match self {
            FutexDeadline::UserClock(deadline_us) => get_time_us() >= deadline_us,
            FutexDeadline::TimeoutClock(deadline_us) => get_timeout_us() >= deadline_us,
        }
    }
}

struct FutexWait {
    deadline: Option<FutexDeadline>,
    completion: WaitCompletion,
}

struct FutexWaits {
    waits: BTreeMap<usize, FutexWait>,
    user_deadlines: BTreeMap<usize, Vec<usize>>,
    timeout_deadlines: BTreeMap<usize, Vec<usize>>,
}

impl FutexWaits {
    fn new() -> Self {
        Self {
            waits: BTreeMap::new(),
            user_deadlines: BTreeMap::new(),
            timeout_deadlines: BTreeMap::new(),
        }
    }

    fn register(&mut self, tid: usize, deadline: Option<FutexDeadline>) {
        self.cancel(tid);
        self.waits.insert(
            tid,
            FutexWait {
                deadline,
                completion: WaitCompletion::Pending,
            },
        );
        let Some(deadline) = deadline else {
            return;
        };
        let deadlines = match deadline {
            FutexDeadline::UserClock(_) => &mut self.user_deadlines,
            FutexDeadline::TimeoutClock(_) => &mut self.timeout_deadlines,
        };
        deadlines.entry(deadline.micros()).or_default().push(tid);
    }

    fn complete(&mut self, tid: usize, completion: WaitCompletion) -> bool {
        debug_assert_ne!(completion, WaitCompletion::Pending);
        let deadline = {
            let Some(wait) = self.waits.get_mut(&tid) else {
                return false;
            };
            if wait.completion != WaitCompletion::Pending {
                return false;
            }
            wait.completion = completion;
            wait.deadline
        };
        if let Some(deadline) = deadline {
            self.remove_deadline(tid, deadline);
        }
        true
    }

    fn finish(&mut self, tid: usize) -> Option<WaitCompletion> {
        let Some(wait) = self.waits.remove(&tid) else {
            return None;
        };
        if let Some(deadline) = wait.deadline {
            self.remove_deadline(tid, deadline);
        }
        Some(wait.completion)
    }

    fn cancel(&mut self, tid: usize) -> (bool, bool) {
        let Some(wait) = self.waits.remove(&tid) else {
            return (false, false);
        };
        let had_deadline = wait.deadline.is_some();
        if let Some(deadline) = wait.deadline {
            self.remove_deadline(tid, deadline);
        }
        (true, had_deadline)
    }

    fn remove_deadline(&mut self, tid: usize, deadline: FutexDeadline) {
        let (deadlines, deadline_us) = match deadline {
            FutexDeadline::UserClock(us) => (&mut self.user_deadlines, us),
            FutexDeadline::TimeoutClock(us) => (&mut self.timeout_deadlines, us),
        };
        let remove_bucket = if let Some(tids) = deadlines.get_mut(&deadline_us) {
            tids.retain(|queued_tid| *queued_tid != tid);
            tids.is_empty()
        } else {
            false
        };
        if remove_bucket {
            deadlines.remove(&deadline_us);
        }
    }

    fn expire(&mut self, now_user: usize, now_timeout: usize) -> Vec<usize> {
        let mut expired = Vec::new();
        Self::expire_clock(
            &mut self.waits,
            &mut self.user_deadlines,
            now_user,
            true,
            &mut expired,
        );
        Self::expire_clock(
            &mut self.waits,
            &mut self.timeout_deadlines,
            now_timeout,
            false,
            &mut expired,
        );
        expired
    }

    fn expire_clock(
        waits: &mut BTreeMap<usize, FutexWait>,
        deadlines: &mut BTreeMap<usize, Vec<usize>>,
        now: usize,
        user_clock: bool,
        expired: &mut Vec<usize>,
    ) {
        while let Some((&deadline_us, _)) = deadlines.first_key_value() {
            if deadline_us > now {
                break;
            }
            let tids = deadlines.remove(&deadline_us).unwrap_or_default();
            for tid in tids {
                let Some(wait) = waits.get_mut(&tid) else {
                    continue;
                };
                let same_deadline = match wait.deadline {
                    Some(FutexDeadline::UserClock(us)) => user_clock && us == deadline_us,
                    Some(FutexDeadline::TimeoutClock(us)) => !user_clock && us == deadline_us,
                    None => false,
                };
                if same_deadline && wait.completion == WaitCompletion::Pending {
                    wait.completion = WaitCompletion::TimedOut;
                    wait.deadline = None;
                    expired.push(tid);
                }
            }
        }
    }
}

lazy_static! {
    static ref FUTEX_WAITS: SpinNoIrqLock<FutexWaits> = SpinNoIrqLock::new(FutexWaits::new());
}

fn read_futex_value(uaddr: usize) -> SysResult<u32> {
    let mut val: u32 = 0;
    copy_from_user(&mut val as *mut u32, uaddr as *const u32, 1)?;
    Ok(val)
}

fn futex_key(uaddr: usize, private: bool) -> SysResult<FutexKey> {
    let task = current_task().expect("no current task");
    if private {
        return Ok(FutexKey {
            scope: task.tgid(),
            uaddr,
        });
    }

    if let Ok(key) =
        task.op_memory_set_read(|memory_set| memory_set.shared_futex_key(VirtAddr::from(uaddr)))
    {
        return Ok(FutexKey {
            scope: key.owner,
            uaddr: key.page_index * PAGE_SIZE + key.offset,
        });
    }

    Ok(FutexKey {
        scope: task.tgid(),
        uaddr,
    })
}

fn trace_futex(op: &str, key: &FutexKey, val: u32, extra: usize) {
    if FUTEX_TRACE {
        if let Some(task) = current_task() {
            println!(
                "[futex-trace] op={} tid={} tgid={} scope={} uaddr={:#x} val={} extra={}",
                op,
                task.tid(),
                task.tgid(),
                key.scope,
                key.uaddr,
                val,
                extra
            );
        }
    }
}

fn futex_wait_common(
    uaddr: usize,
    expected_val: u32,
    bitset: u32,
    private: bool,
) -> SysResult<usize> {
    if bitset == 0 {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");
    let key = futex_key(uaddr, private)?;
    let hash_idx = futex_hash_idx(&key);

    // Resolve lazy/COW mappings before entering the global queue critical
    // section.  The value is read again under FUTEX_QUEUES below so the
    // compare-and-enqueue operation remains atomic with respect to wakeups.
    check_user_readable(uaddr as *const u32, 1)?;

    {
        let mut queues = FUTEX_QUEUES.lock();
        let actual_val = read_user_u32_nofault(uaddr as *const u32)?;
        if actual_val != expected_val {
            trace_futex("wait-eagain", &key, expected_val, actual_val as usize);
            return Err(Errno::EAGAIN);
        }

        // From here until the task is woken, signal delivery must be able to
        // interrupt this futex wait. Set this before enqueueing so a cancel
        // signal cannot arrive in the window before the task is blocked.
        task.set_interruptible(true);

        queues.bucket_by_idx(hash_idx).push_back(FutexQ {
            key: key.clone(),
            tid: task.tid(),
            bitset,
        });
        register_futex_wait(task.tid(), None);

        if task.check_signal_interrupt() {
            task.clear_interrupted();
            task.set_interruptible(false);
            complete_futex_wait(task.tid(), WaitCompletion::Interrupted);
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
            finish_futex_wait(task.tid());
            trace_futex(
                "wait-eintr-before-block",
                &key,
                expected_val,
                bitset as usize,
            );
            return Err(Errno::EINTR);
        }

        if !prepare_current_task_blocked() {
            task.set_interruptible(false);
            cancel_futex_wait(task.tid());
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
            trace_futex("wait-no-runner", &key, expected_val, bitset as usize);
            return Err(Errno::EAGAIN);
        }

        let interrupted = task.is_interrupted() || task.check_signal_interrupt();
        if interrupted {
            task.clear_interrupted();
            task.set_interruptible(false);
            complete_futex_wait(task.tid(), WaitCompletion::Interrupted);
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
            finish_futex_wait(task.tid());
            drop(queues);
            wakeup_task(task.tid());
            remove_task(task.tid());
            task.set_running();
            trace_futex(
                "wait-eintr-after-block",
                &key,
                expected_val,
                bitset as usize,
            );
            return Err(Errno::EINTR);
        }
    }

    trace_futex("wait", &key, expected_val, bitset as usize);

    switch_to_next_task();
    task.set_interruptible(false);
    let completion = finish_futex_wait(task.tid()).unwrap_or(WaitCompletion::Pending);
    task.clear_interrupted();

    // A winner normally removes the queue entry before waking the task. Keep
    // this cleanup for a scheduler-level spurious wake.
    FUTEX_QUEUES
        .lock()
        .bucket_by_idx(hash_idx)
        .retain(|q| !(q.tid == task.tid() && q.key == key));

    match completion {
        WaitCompletion::Interrupted => {
            trace_futex("wait-eintr", &key, expected_val, bitset as usize);
            Err(Errno::EINTR)
        }
        WaitCompletion::TimedOut => Err(Errno::ETIMEDOUT),
        WaitCompletion::Woken | WaitCompletion::Pending => Ok(0),
    }
}

fn register_futex_wait(tid: usize, deadline: Option<FutexDeadline>) {
    FUTEX_WAITS.lock().register(tid, deadline);
}

fn complete_futex_wait(tid: usize, completion: WaitCompletion) -> bool {
    FUTEX_WAITS.lock().complete(tid, completion)
}

fn finish_futex_wait(tid: usize) -> Option<WaitCompletion> {
    FUTEX_WAITS.lock().finish(tid)
}

fn cancel_futex_wait(tid: usize) {
    let _ = FUTEX_WAITS.lock().cancel(tid);
}

pub fn check_futex_timeouts() {
    let expired = {
        // Lock order: FUTEX_QUEUES -> FUTEX_WAITS -> SCHEDULER.
        let mut queues = FUTEX_QUEUES.lock();
        let expired = FUTEX_WAITS.lock().expire(get_time_us(), get_timeout_us());
        for tid in &expired {
            queues.remove_tid(*tid);
        }
        expired
    };

    for tid in expired {
        wakeup_task(tid);
    }
}

/// Claim signal interruption for a futex waiter and remove its queue entry.
///
/// Signal delivery still owns the scheduler wakeup because it also handles
/// non-futex interruptible waits.
pub fn interrupt_futex_wait(tid: usize) -> bool {
    let mut queues = FUTEX_QUEUES.lock();
    if !complete_futex_wait(tid, WaitCompletion::Interrupted) {
        return false;
    }
    queues.remove_tid(tid);
    true
}

/// Remove all futex wait state for a task that will never resume.
pub fn remove_futex_waiter(tid: usize) {
    let mut queues = FUTEX_QUEUES.lock();
    let removed_queue = queues.remove_tid(tid);
    let (removed_wait, removed_deadline) = FUTEX_WAITS.lock().cancel(tid);
    if FUTEX_EXIT_TRACE && (removed_queue != 0 || removed_wait) {
        println!(
            "[futex-exit-trace] tid={} queue={} wait={} deadline={}",
            tid, removed_queue, removed_wait, removed_deadline
        );
    }
}

fn futex_deadline_us(
    timeout_ptr: usize,
    absolute: bool,
    realtime: bool,
) -> SysResult<Option<FutexDeadline>> {
    if timeout_ptr == 0 {
        return Ok(None);
    }

    let mut timeout = TimeSpec::default();
    copy_from_user(
        &mut timeout as *mut TimeSpec,
        timeout_ptr as *const TimeSpec,
        1,
    )?;
    let timeout_us = timeout.checked_duration_us().ok_or(Errno::EINVAL)?;
    if absolute {
        Ok(Some(if realtime {
            FutexDeadline::UserClock(timeout_us)
        } else {
            FutexDeadline::TimeoutClock(timeout_us)
        }))
    } else {
        Ok(Some(FutexDeadline::TimeoutClock(
            get_timeout_us()
                .checked_add(timeout_us)
                .ok_or(Errno::EINVAL)?,
        )))
    }
}

fn futex_wait_timed_common(
    uaddr: usize,
    expected_val: u32,
    bitset: u32,
    deadline: Option<FutexDeadline>,
    private: bool,
) -> SysResult<usize> {
    let Some(deadline) = deadline else {
        return futex_wait_common(uaddr, expected_val, bitset, private);
    };
    if bitset == 0 {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");
    let key = futex_key(uaddr, private)?;
    let hash_idx = futex_hash_idx(&key);
    loop {
        let actual_val = read_futex_value(uaddr)?;
        if actual_val != expected_val {
            trace_futex(
                "wait-timed-changed",
                &key,
                expected_val,
                actual_val as usize,
            );
            return Err(Errno::EAGAIN);
        }
        if deadline.expired() {
            trace_futex("wait-timedout", &key, expected_val, bitset as usize);
            return Err(Errno::ETIMEDOUT);
        }
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            trace_futex("wait-timed-eintr", &key, expected_val, bitset as usize);
            return Err(Errno::EINTR);
        }

        {
            let mut queues = FUTEX_QUEUES.lock();
            // The faulting read above resolved the mapping.  Do not call
            // copy_from_user while holding FUTEX_QUEUES: that needs the
            // MemorySet write lock and can deadlock against another hart.
            let actual_val = read_user_u32_nofault(uaddr as *const u32)?;
            if actual_val != expected_val {
                trace_futex(
                    "wait-timed-changed",
                    &key,
                    expected_val,
                    actual_val as usize,
                );
                return Err(Errno::EAGAIN);
            }

            task.set_interruptible(true);
            queues.bucket_by_idx(hash_idx).push_back(FutexQ {
                key: key.clone(),
                tid: task.tid(),
                bitset,
            });
            register_futex_wait(task.tid(), Some(deadline));

            if task.check_signal_interrupt() {
                task.clear_interrupted();
                task.set_interruptible(false);
                complete_futex_wait(task.tid(), WaitCompletion::Interrupted);
                queues
                    .bucket_by_idx(hash_idx)
                    .retain(|q| !(q.tid == task.tid() && q.key == key));
                finish_futex_wait(task.tid());
                trace_futex(
                    "wait-timed-eintr-before-block",
                    &key,
                    expected_val,
                    bitset as usize,
                );
                return Err(Errno::EINTR);
            }

            if !prepare_current_task_blocked() {
                task.set_interruptible(false);
                cancel_futex_wait(task.tid());
                queues
                    .bucket_by_idx(hash_idx)
                    .retain(|q| !(q.tid == task.tid() && q.key == key));
                drop(queues);
                yield_current_task();
                continue;
            }

            let interrupted = task.is_interrupted() || task.check_signal_interrupt();
            if interrupted {
                task.clear_interrupted();
                task.set_interruptible(false);
                complete_futex_wait(task.tid(), WaitCompletion::Interrupted);
                queues
                    .bucket_by_idx(hash_idx)
                    .retain(|q| !(q.tid == task.tid() && q.key == key));
                finish_futex_wait(task.tid());
                drop(queues);
                wakeup_task(task.tid());
                remove_task(task.tid());
                task.set_running();
                trace_futex(
                    "wait-timed-eintr-after-block",
                    &key,
                    expected_val,
                    bitset as usize,
                );
                return Err(Errno::EINTR);
            }
        }

        switch_to_next_task();
        task.set_interruptible(false);

        let completion = finish_futex_wait(task.tid()).unwrap_or(WaitCompletion::Pending);
        task.clear_interrupted();

        FUTEX_QUEUES
            .lock()
            .bucket_by_idx(hash_idx)
            .retain(|q| !(q.tid == task.tid() && q.key == key));
        return match completion {
            WaitCompletion::Interrupted => {
                trace_futex("wait-timed-eintr", &key, expected_val, bitset as usize);
                Err(Errno::EINTR)
            }
            WaitCompletion::TimedOut => {
                trace_futex("wait-timedout", &key, expected_val, bitset as usize);
                Err(Errno::ETIMEDOUT)
            }
            WaitCompletion::Woken | WaitCompletion::Pending => {
                trace_futex("wait-timed-woken", &key, expected_val, bitset as usize);
                Ok(0)
            }
        };
    }
}

fn futex_wake_common(uaddr: usize, nr_wake: u32, bitset: u32, private: bool) -> SysResult<usize> {
    if bitset == 0 {
        return Err(Errno::EINVAL);
    }

    let key = futex_key(uaddr, private)?;
    let hash_idx = futex_hash_idx(&key);
    let mut woken_tids = Vec::new();

    {
        let mut queues = FUTEX_QUEUES.lock();
        let bucket = queues.bucket_by_idx(hash_idx);
        let mut i = 0;
        while i < bucket.len() && woken_tids.len() < nr_wake as usize {
            if bucket[i].key == key && (bucket[i].bitset & bitset) != 0 {
                let futex_q = bucket.remove(i).unwrap();
                if complete_futex_wait(futex_q.tid, WaitCompletion::Woken) {
                    woken_tids.push(futex_q.tid);
                }
            } else {
                i += 1;
            }
        }
    }

    let woken = woken_tids.len();
    for tid in woken_tids {
        wakeup_task(tid);
    }

    trace_futex("wake", &key, nr_wake, woken);
    Ok(woken)
}

fn futex_requeue_common(
    uaddr: usize,
    nr_wake: u32,
    uaddr2: usize,
    nr_requeue: u32,
    expected_val: Option<u32>,
    private: bool,
) -> SysResult<usize> {
    if uaddr == 0 || uaddr2 == 0 {
        return Err(Errno::EINVAL);
    }

    let source_key = futex_key(uaddr, private)?;
    let target_key = futex_key(uaddr2, private)?;

    // Resolve a lazy user page before taking the queue spin lock. The final
    // value load still happens under FUTEX_QUEUES below, so comparison and
    // queue mutation share one linearization interval without allowing page
    // allocation/fault handling while the no-IRQ lock is held.
    if expected_val.is_some() {
        check_user_readable(uaddr as *const u32, 1)?;
        if let (true, Some(expected_val)) = (FUTEX_CMP_REQUEUE_TEST_YIELD, expected_val) {
            while read_futex_value(uaddr)? == expected_val {
                yield_current_task();
            }
        }
    }

    let source_idx = futex_hash_idx(&source_key);
    let target_idx = futex_hash_idx(&target_key);
    let mut moved = Vec::new();
    let mut woken_tids = Vec::new();
    let mut affected = 0usize;
    let mut requeued = 0usize;

    {
        let mut queues = FUTEX_QUEUES.lock();
        if let Some(expected_val) = expected_val {
            // Lock order: FUTEX_QUEUES -> MemorySet(read) -> FUTEX_WAITS.
            // The page was resolved above. This fixed-size no-fault read only
            // translates the existing PTE; it cannot allocate or block on
            // page-fault handling while the queue lock is held.
            let actual_val = read_user_u32_nofault(uaddr as *const u32)?;
            if actual_val != expected_val {
                return Err(Errno::EAGAIN);
            }
        }

        let source_bucket = queues.bucket_by_idx(source_idx);
        let mut idx = 0;
        while idx < source_bucket.len() && woken_tids.len() < nr_wake as usize {
            if source_bucket[idx].key == source_key {
                let futex_q = source_bucket.remove(idx).unwrap();
                if complete_futex_wait(futex_q.tid, WaitCompletion::Woken) {
                    woken_tids.push(futex_q.tid);
                    affected += 1;
                }
            } else {
                idx += 1;
            }
        }

        while source_key != target_key
            && idx < source_bucket.len()
            && requeued < nr_requeue as usize
        {
            if source_bucket[idx].key == source_key {
                let mut futex_q = source_bucket.remove(idx).unwrap();
                futex_q.key = target_key.clone();
                moved.push(futex_q);
                requeued += 1;
                affected += 1;
            } else {
                idx += 1;
            }
        }

        if !moved.is_empty() {
            let target_bucket = queues.bucket_by_idx(target_idx);
            for futex_q in moved {
                target_bucket.push_back(futex_q);
            }
        }
    }

    for tid in woken_tids {
        wakeup_task(tid);
    }

    Ok(affected)
}

pub fn futex_wait(
    uaddr: usize,
    expected_val: u32,
    timeout_ptr: usize,
    private: bool,
) -> SysResult<usize> {
    let deadline_us = futex_deadline_us(timeout_ptr, false, false)?;
    futex_wait_timed_common(
        uaddr,
        expected_val,
        FUTEX_BITSET_MATCH_ANY,
        deadline_us,
        private,
    )
}

pub fn futex_wake(uaddr: usize, nr_wake: u32, private: bool) -> SysResult<usize> {
    futex_wake_common(uaddr, nr_wake, FUTEX_BITSET_MATCH_ANY, private)
}

pub fn futex_requeue(
    uaddr: usize,
    nr_wake: u32,
    uaddr2: usize,
    nr_requeue: u32,
    private: bool,
) -> SysResult<usize> {
    futex_requeue_common(uaddr, nr_wake, uaddr2, nr_requeue, None, private)
}

pub fn futex_cmp_requeue(
    uaddr: usize,
    nr_wake: u32,
    uaddr2: usize,
    nr_requeue: u32,
    expected_val: u32,
    private: bool,
) -> SysResult<usize> {
    futex_requeue_common(
        uaddr,
        nr_wake,
        uaddr2,
        nr_requeue,
        Some(expected_val),
        private,
    )
}

pub fn futex_wait_bitset(
    uaddr: usize,
    expected_val: u32,
    timeout_ptr: usize,
    bitset: u32,
    absolute_timeout: bool,
    realtime: bool,
    private: bool,
) -> SysResult<usize> {
    let deadline_us = futex_deadline_us(timeout_ptr, absolute_timeout, realtime)?;
    futex_wait_timed_common(uaddr, expected_val, bitset, deadline_us, private)
}

pub fn futex_wake_bitset(
    uaddr: usize,
    nr_wake: u32,
    bitset: u32,
    private: bool,
) -> SysResult<usize> {
    futex_wake_common(uaddr, nr_wake, bitset, private)
}

pub fn futex_wake_private(uaddr: usize, nr_wake: u32) -> SysResult<usize> {
    futex_wake(uaddr, nr_wake, true)
}
