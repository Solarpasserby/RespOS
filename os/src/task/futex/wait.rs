// os/src/task/futex/wait.rs

use super::queue::{FUTEX_QUEUES, FutexKey, FutexQ, futex_hash_idx};
use crate::mm::copy_from_user;
use crate::syscall::{Errno, SysResult};
use crate::task::scheduler::{prepare_current_task_blocked, switch_to_next_task, wakeup_task};
use crate::task::{current_task, futex::FUTEX_BITSET_MATCH_ANY};

fn read_futex_value(uaddr: usize) -> SysResult<u32> {
    let mut val: u32 = 0;
    copy_from_user(&mut val as *mut u32, uaddr as *const u32, 1)?;
    Ok(val)
}

pub fn futex_wait(uaddr: usize, expected_val: u32) -> SysResult<usize> {
    let task = current_task().expect("no current task");

    let key = FutexKey {
        mm_token: task.get_user_token(),
        uaddr,
    };

    let hash_idx = futex_hash_idx(uaddr);
    {
        let mut queues = FUTEX_QUEUES.lock();
        let actual_val = read_futex_value(uaddr)?;
        if actual_val != expected_val {
            return Err(Errno::EAGAIN);
        }

        queues.bucket_by_idx(hash_idx).push_back(FutexQ {
            key: key.clone(),
            tid: task.tid(),
            bitset: FUTEX_BITSET_MATCH_ANY,
        });

        if !prepare_current_task_blocked() {
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
            return Err(Errno::EAGAIN);
        }
    }

    switch_to_next_task();

    // 被唤醒后，清理队列中自己的条目
    let mut queues = FUTEX_QUEUES.lock();
    queues
        .bucket_by_idx(hash_idx)
        .retain(|q| !(q.tid == task.tid() && q.key == key));

    Ok(0)
}

pub fn futex_wake(uaddr: usize, nr_wake: u32) -> SysResult<usize> {
    let task = current_task().expect("no current task");
    let key = FutexKey {
        mm_token: task.get_user_token(),
        uaddr,
    };
    let hash_idx = futex_hash_idx(uaddr);
    let mut queues = FUTEX_QUEUES.lock();
    let bucket = queues.bucket_by_idx(hash_idx);
    let mut woken = 0usize;

    let mut i = 0;
    while i < bucket.len() && woken < nr_wake as usize {
        if bucket[i].key == key {
            let futex_q = bucket.remove(i).unwrap();
            wakeup_task(futex_q.tid);
            woken += 1;
        } else {
            i += 1;
        }
    }

    Ok(woken)
}

pub fn futex_wait_bitset(uaddr: usize, expected_val: u32, bitset: u32) -> SysResult<usize> {
    if bitset == 0 {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");

    let key = FutexKey {
        mm_token: task.get_user_token(),
        uaddr,
    };

    let hash_idx = futex_hash_idx(uaddr);
    {
        let mut queues = FUTEX_QUEUES.lock();
        let actual_val = read_futex_value(uaddr)?;
        if actual_val != expected_val {
            return Err(Errno::EAGAIN);
        }

        queues.bucket_by_idx(hash_idx).push_back(FutexQ {
            key: key.clone(),
            tid: task.tid(),
            bitset,
        });

        if !prepare_current_task_blocked() {
            queues
                .bucket_by_idx(hash_idx)
                .retain(|q| !(q.tid == task.tid() && q.key == key));
            return Err(Errno::EAGAIN);
        }
    }

    switch_to_next_task();

    let mut queues = FUTEX_QUEUES.lock();
    queues
        .bucket_by_idx(hash_idx)
        .retain(|q| !(q.tid == task.tid() && q.key == key));

    Ok(0)
}

pub fn futex_wake_bitset(uaddr: usize, nr_wake: u32, bitset: u32) -> SysResult<usize> {
    if bitset == 0 {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("no current task");
    let key = FutexKey {
        mm_token: task.get_user_token(),
        uaddr,
    };
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

    Ok(woken)
}
