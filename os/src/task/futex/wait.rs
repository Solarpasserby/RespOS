// os/src/task/futex/wait.rs

use super::queue::{FUTEX_QUEUES, FutexKey, FutexQ, futex_hash_idx};
use crate::mm::copy_from_user;
use crate::syscall::{Errno, SysResult};
use crate::task::scheduler::{
    prepare_current_task_blocked, remove_task, switch_to_next_task, wakeup_task,
};
use crate::task::{current_task, futex::FUTEX_BITSET_MATCH_ANY};
use alloc::vec::Vec;

const FUTEX_TRACE: bool = false;

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

fn futex_wake_common(uaddr: usize, nr_wake: u32, bitset: u32, private: bool) -> SysResult<usize> {
    if bitset == 0 {
        return Err(Errno::EINVAL);
    }

    let key = futex_key(uaddr, private);
    let hash_idx = futex_hash_idx(uaddr);
    let mut queues = FUTEX_QUEUES.lock();
    let bucket = queues.bucket_by_idx(hash_idx);
    let mut woken = 0usize;

    let mut i = 0;
    while i < bucket.len() && woken < nr_wake as usize {
        if bucket[i].key == key && (bucket[i].bitset & bitset) != 0 {
            let futex_q = bucket.remove(i).unwrap();
            wakeup_task(futex_q.tid);
            woken += 1;
        } else {
            i += 1;
        }
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
    let mut queues = FUTEX_QUEUES.lock();
    let mut moved = Vec::new();
    let mut affected = 0usize;
    let mut woken = 0usize;
    let mut requeued = 0usize;

    {
        let source_bucket = queues.bucket_by_idx(source_idx);
        let mut idx = 0;
        while idx < source_bucket.len() && woken < nr_wake as usize {
            if source_bucket[idx].key == source_key {
                let futex_q = source_bucket.remove(idx).unwrap();
                wakeup_task(futex_q.tid);
                woken += 1;
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
    }

    if !moved.is_empty() {
        let target_bucket = queues.bucket_by_idx(target_idx);
        for futex_q in moved {
            target_bucket.push_back(futex_q);
        }
    }

    Ok(affected)
}

pub fn futex_wait(uaddr: usize, expected_val: u32, private: bool) -> SysResult<usize> {
    futex_wait_common(uaddr, expected_val, FUTEX_BITSET_MATCH_ANY, private)
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
    bitset: u32,
    private: bool,
) -> SysResult<usize> {
    futex_wait_common(uaddr, expected_val, bitset, private)
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
