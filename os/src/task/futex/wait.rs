// os/src/task/futex/wait.rs

use super::queue::{FUTEX_QUEUES, FutexKey, FutexQ, futex_hash_idx};
use crate::mm::copy_from_user;
use crate::mutex::SpinNoIrqLock;
use crate::syscall::{Errno, SysResult};
use crate::task::scheduler::{
    prepare_current_task_blocked, remove_task, switch_to_next_task, wakeup_task,
};
use crate::task::{current_task, futex::FUTEX_BITSET_MATCH_ANY, yield_current_task};
use crate::timer::{TimeSpec, get_time_ms, get_timeout_ms};
use alloc::vec::Vec;
use lazy_static::lazy_static;

const FUTEX_TRACE: bool = false;

struct TimedFutexWait {
    tid: usize,
    deadline: FutexDeadline,
    timed_out: bool,
}

lazy_static! {
    static ref TIMED_FUTEX_WAITS: SpinNoIrqLock<Vec<TimedFutexWait>> =
        SpinNoIrqLock::new(Vec::new());
}

fn read_futex_value(uaddr: usize) -> SysResult<u32> {
    let mut val: u32 = 0;
    copy_from_user(&mut val as *mut u32, uaddr as *const u32, 1)?;
    Ok(val)
}

fn futex_key(uaddr: usize, private: bool) -> FutexKey {
    let scope = if private {
        current_task().expect("no current task").tgid()
    } else {
        0
    };
    FutexKey { scope, uaddr }
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
    let key = futex_key(uaddr, private);
    let hash_idx = futex_hash_idx(uaddr);

    {
        let mut queues = FUTEX_QUEUES.lock();
        let actual_val = read_futex_value(uaddr)?;
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

        if task.check_signal_interrupt() {
            task.clear_interrupted();
            task.set_interruptible(false);
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
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
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
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
    // ★ 醒来后检查：是 futex_wake 叫醒的，还是信号打断的？
    if task.is_interrupted() {
        task.clear_interrupted();
        let mut queues = FUTEX_QUEUES.lock();
        queues
            .bucket_by_idx(hash_idx)
            .retain(|q| !(q.tid == task.tid() && q.key == key));
        trace_futex("wait-eintr", &key, expected_val, bitset as usize);
        return Err(Errno::EINTR);
    }

    // 正常路径：被 futex_wake 唤醒
    let mut queues = FUTEX_QUEUES.lock();
    queues
        .bucket_by_idx(hash_idx)
        .retain(|q| !(q.tid == task.tid() && q.key == key));

    Ok(0)
}

#[derive(Clone, Copy)]
enum FutexDeadline {
    UserClock(usize),
    TimeoutClock(usize),
}

impl FutexDeadline {
    fn expired(self) -> bool {
        match self {
            FutexDeadline::UserClock(deadline_ms) => get_time_ms() >= deadline_ms,
            FutexDeadline::TimeoutClock(deadline_ms) => get_timeout_ms() >= deadline_ms,
        }
    }
}

fn register_timed_wait(tid: usize, deadline: FutexDeadline) {
    TIMED_FUTEX_WAITS.lock().push(TimedFutexWait {
        tid,
        deadline,
        timed_out: false,
    });
}

fn finish_timed_wait(tid: usize) -> bool {
    let mut waits = TIMED_FUTEX_WAITS.lock();
    if let Some(pos) = waits.iter().position(|wait| wait.tid == tid) {
        waits.remove(pos).timed_out
    } else {
        false
    }
}

pub fn check_futex_timeouts() {
    let mut expired = Vec::new();
    {
        let mut waits = TIMED_FUTEX_WAITS.lock();
        for wait in waits.iter_mut() {
            if !wait.timed_out && wait.deadline.expired() {
                wait.timed_out = true;
                expired.push(wait.tid);
            }
        }
    }

    for tid in expired {
        FUTEX_QUEUES.lock().remove_tid(tid);
        wakeup_task(tid);
    }
}

fn futex_deadline_ms(timeout_ptr: usize, absolute: bool) -> SysResult<Option<FutexDeadline>> {
    if timeout_ptr == 0 {
        return Ok(None);
    }

    let mut timeout = TimeSpec::default();
    copy_from_user(
        &mut timeout as *mut TimeSpec,
        timeout_ptr as *const TimeSpec,
        1,
    )?;
    let timeout_ms = timeout.checked_duration_ms().ok_or(Errno::EINVAL)?;
    if absolute {
        Ok(Some(FutexDeadline::UserClock(timeout_ms)))
    } else {
        Ok(Some(FutexDeadline::TimeoutClock(
            get_timeout_ms()
                .checked_add(timeout_ms)
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
    let key = futex_key(uaddr, private);
    let hash_idx = futex_hash_idx(uaddr);
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

            if !prepare_current_task_blocked() {
                drop(queues);
                task.set_interruptible(true);
                yield_current_task();
                task.set_interruptible(false);
                continue;
            }

            queues.bucket_by_idx(hash_idx).push_back(FutexQ {
                key: key.clone(),
                tid: task.tid(),
                bitset,
            });
        }

        task.set_interruptible(true);
        register_timed_wait(task.tid(), deadline);
        switch_to_next_task();
        task.set_interruptible(false);

        if task.is_interrupted() {
            task.clear_interrupted();
            finish_timed_wait(task.tid());
            let mut queues = FUTEX_QUEUES.lock();
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
            trace_futex("wait-timed-eintr", &key, expected_val, bitset as usize);
            return Err(Errno::EINTR);
        }

        if finish_timed_wait(task.tid()) {
            trace_futex("wait-timedout", &key, expected_val, bitset as usize);
            return Err(Errno::ETIMEDOUT);
        }

        FUTEX_QUEUES
            .lock()
            .bucket_by_idx(hash_idx)
            .retain(|q| !(q.tid == task.tid() && q.key == key));
        trace_futex("wait-timed-woken", &key, expected_val, bitset as usize);
        return Ok(0);
    }
}

fn futex_wake_common(uaddr: usize, nr_wake: u32, bitset: u32, private: bool) -> SysResult<usize> {
    if bitset == 0 {
        return Err(Errno::EINVAL);
    }

    let key = futex_key(uaddr, private);
    let hash_idx = futex_hash_idx(uaddr);
    let mut woken_tids = Vec::new();

    {
        let mut queues = FUTEX_QUEUES.lock();
        let bucket = queues.bucket_by_idx(hash_idx);
        let mut i = 0;
        while i < bucket.len() && woken_tids.len() < nr_wake as usize {
            if bucket[i].key == key && (bucket[i].bitset & bitset) != 0 {
                let futex_q = bucket.remove(i).unwrap();
                woken_tids.push(futex_q.tid);
            } else {
                i += 1;
            }
        }
    }

    let woken = woken_tids.len();
    for tid in woken_tids {
        finish_timed_wait(tid);
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
    private: bool,
) -> SysResult<usize> {
    if uaddr == 0 || uaddr2 == 0 {
        return Err(Errno::EINVAL);
    }

    let source_key = futex_key(uaddr, private);
    let target_key = futex_key(uaddr2, private);

    if source_key == target_key {
        return futex_wake(uaddr, nr_wake, private);
    }

    let source_idx = futex_hash_idx(uaddr);
    let target_idx = futex_hash_idx(uaddr2);
    let mut moved = Vec::new();
    let mut woken_tids = Vec::new();
    let mut affected = 0usize;
    let mut requeued = 0usize;

    {
        let mut queues = FUTEX_QUEUES.lock();
        let source_bucket = queues.bucket_by_idx(source_idx);
        let mut idx = 0;
        while idx < source_bucket.len() && woken_tids.len() < nr_wake as usize {
            if source_bucket[idx].key == source_key {
                let futex_q = source_bucket.remove(idx).unwrap();
                woken_tids.push(futex_q.tid);
                affected += 1;
            } else {
                idx += 1;
            }
        }

        while idx < source_bucket.len() && requeued < nr_requeue as usize {
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
        finish_timed_wait(tid);
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
    let deadline_ms = futex_deadline_ms(timeout_ptr, false)?;
    futex_wait_timed_common(
        uaddr,
        expected_val,
        FUTEX_BITSET_MATCH_ANY,
        deadline_ms,
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
    futex_requeue_common(uaddr, nr_wake, uaddr2, nr_requeue, private)
}

pub fn futex_cmp_requeue(
    uaddr: usize,
    nr_wake: u32,
    uaddr2: usize,
    nr_requeue: u32,
    expected_val: u32,
    private: bool,
) -> SysResult<usize> {
    let actual_val = read_futex_value(uaddr)?;
    if actual_val != expected_val {
        return Err(Errno::EAGAIN);
    }
    futex_requeue_common(uaddr, nr_wake, uaddr2, nr_requeue, private)
}

pub fn futex_wait_bitset(
    uaddr: usize,
    expected_val: u32,
    timeout_ptr: usize,
    bitset: u32,
    absolute_timeout: bool,
    private: bool,
) -> SysResult<usize> {
    let deadline_ms = futex_deadline_ms(timeout_ptr, absolute_timeout)?;
    futex_wait_timed_common(uaddr, expected_val, bitset, deadline_ms, private)
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
