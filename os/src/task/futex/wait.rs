// os/src/task/futex/wait.rs

//! futex WAIT/WAKE/REQUEUE 等等待操作的线性化实现。
//!
//! futex 的身份不是裸用户虚拟地址：私有 futex 按地址空间区分，共享 futex 必须解析到
//! 可跨进程一致的 backing key。等待者进入 hash queue 前后都要验证用户值，采用“读值 →
//! 加队列锁 → 再读值 → 入队”的顺序关闭 lost wake 窗口。
//!
//! 一个 waiter 只能由 wake、timeout、signal 或退出清理中的一个路径完成，`WaitCompletion`
//! 记录 single-winner 结果。完成路径必须对称移除 queue entry、deadline 和 task blocked
//! 状态；requeue 同时涉及两个 bucket 时要使用稳定锁序。用户地址读取使用 nofault/copy
//! helper，不能持有 futex 队列锁触发会递归进入 MM 或调度器的缺页处理。

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

#[cfg(feature = "debug_traces")]
const FUTEX_TRACE: bool = option_env!("TASK_A_FUTEX_TRACE").is_some();
#[cfg(feature = "debug_traces")]
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

    fn remaining_us(self) -> usize {
        match self {
            FutexDeadline::UserClock(deadline_us) => deadline_us.saturating_sub(get_time_us()),
            FutexDeadline::TimeoutClock(deadline_us) => {
                deadline_us.saturating_sub(get_timeout_us())
            }
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

    fn next_deadlines(&self) -> [Option<FutexDeadline>; 2] {
        [
            self.user_deadlines
                .first_key_value()
                .map(|(deadline, _)| FutexDeadline::UserClock(*deadline)),
            self.timeout_deadlines
                .first_key_value()
                .map(|(deadline, _)| FutexDeadline::TimeoutClock(*deadline)),
        ]
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
    #[cfg(feature = "debug_traces")]
    {
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
    #[cfg(not(feature = "debug_traces"))]
    let _ = (op, key, val, extra);
}

/// 执行无超时 futex WAIT 的“比较并入队”事务。
///
/// 先在全局队列锁外解析用户页，随后持有 `FUTEX_QUEUES` 重新读取 futex 字并与期望值比较；
/// 只有仍相等时才登记 waiter、设置 interruptible 并发布 Blocked，因此 wake 不可能插入在
/// 最终比较与入队之间。私有 futex 以地址空间身份构造 key，共享 futex 以物理帧身份构造 key。
///
/// 恢复后由 wake、信号、退出或伪唤醒中的唯一获胜者认领完成状态；函数清理残留队列项和
/// interruptible 标志，并把信号中断映射为 EINTR。调用期间不得在持队列锁时触发用户缺页。
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

    // 进入全局队列临界区前先解析惰性/COW 映射。下方持有 FUTEX_QUEUES 时会再次读取值，
    // 从而保证“比较并入队”相对于唤醒操作仍是原子的。
    check_user_readable(uaddr as *const u32, 1)?;

    {
        let mut queues = FUTEX_QUEUES.lock();
        let actual_val = read_user_u32_nofault(uaddr as *const u32)?;
        if actual_val != expected_val {
            trace_futex("wait-eagain", &key, expected_val, actual_val as usize);
            return Err(Errno::EAGAIN);
        }

        // 从这里到任务被唤醒期间，信号投递必须能够中断 futex 等待。
        // 在入队前设置该状态，避免取消信号落入任务真正阻塞前的竞争窗口。
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

    // 正常情况下，获胜路径会先移除队列项再唤醒任务；这里保留清理逻辑，
    // 用于处理调度器层面的伪唤醒。
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
    if let Some(deadline) = deadline {
        crate::timer::request_task_timer_after_us(deadline.remaining_us());
    }
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
    let (expired, next_deadlines) = {
        // 固定锁顺序：FUTEX_QUEUES → FUTEX_WAITS → SCHEDULER。
        let mut queues = FUTEX_QUEUES.lock();
        let mut waits = FUTEX_WAITS.lock();
        let expired = waits.expire(get_time_us(), get_timeout_us());
        for tid in &expired {
            queues.remove_tid(*tid);
        }
        (expired, waits.next_deadlines())
    };

    for tid in expired {
        wakeup_task(tid);
    }
    for deadline in next_deadlines.into_iter().flatten() {
        crate::timer::request_task_timer_after_us(deadline.remaining_us());
    }
}

/// 为 futex 等待者认领“被信号中断”的结果，并移除其队列项。
///
/// 调度器唤醒仍由信号投递路径负责，因为它还统一处理非 futex 的可中断等待。
pub fn interrupt_futex_wait(tid: usize) -> bool {
    let mut queues = FUTEX_QUEUES.lock();
    if !complete_futex_wait(tid, WaitCompletion::Interrupted) {
        return false;
    }
    queues.remove_tid(tid);
    true
}

/// 移除一个永远不会恢复执行的任务所拥有的全部 futex 等待状态。
pub fn remove_futex_waiter(tid: usize) {
    let mut queues = FUTEX_QUEUES.lock();
    let removed_queue = queues.remove_tid(tid);
    let (removed_wait, removed_deadline) = FUTEX_WAITS.lock().cancel(tid);
    #[cfg(feature = "debug_traces")]
    {
        if FUTEX_EXIT_TRACE && (removed_queue != 0 || removed_wait) {
            println!(
                "[futex-exit-trace] tid={} queue={} wait={} deadline={}",
                tid, removed_queue, removed_wait, removed_deadline
            );
        }
    }
    #[cfg(not(feature = "debug_traces"))]
    let _ = (removed_queue, removed_wait, removed_deadline);
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

/// 执行带绝对/相对期限的 futex WAIT，并协调 wake、signal 与 timeout 三类竞争者。
///
/// 无期限时退化为 `futex_wait_common`；有期限时先注册等待状态和 deadline 索引，再以与
/// 普通 WAIT 相同的锁内二次取值完成原子入队。定时服务、futex wake 和信号路径都只能通过
/// `FUTEX_WAITS` 的完成状态赢得一次，失败者不得再次唤醒或覆盖 errno。
///
/// 返回前统一撤销期限索引、队列项和可中断状态。已到期返回 ETIMEDOUT，值在入队前变化
/// 返回 EAGAIN，信号获胜返回 EINTR；任何路径都不能遗留一个指向已退出任务的 waiter。
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
            // 上面的缺页读取已经解析映射。持有 FUTEX_QUEUES 时不能调用 copy_from_user：
            // 它需要 MemorySet 写锁，可能与另一硬件线程形成死锁。
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
                crate::perf::futex_yield(1);
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

/// 原子唤醒一部分源 futex 等待者，并把其余等待者迁移到目标 futex key。
///
/// `CMP_REQUEUE` 会在队列锁内对源 futex 做最终无缺页读取，比较失败不修改任何队列。
/// 操作始终按 `FUTEX_QUEUES → MemorySet(read) → FUTEX_WAITS` 的固定顺序取锁；迁移时同时
/// 更新每个任务的 waiter 元数据，使之后的信号取消、超时和目标 wake 都定位到新 key。
/// 被选中唤醒的任务先从队列移除并认领完成状态，释放锁后才进入调度器唤醒路径。
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

    // 获取队列自旋锁前先解析惰性用户页。最终取值仍在下方持有 FUTEX_QUEUES 时完成，
    // 因而比较与队列修改共享同一线性化区间，同时避免在持有禁中断锁时分配页面或处理缺页。
    if expected_val.is_some() {
        check_user_readable(uaddr as *const u32, 1)?;
        if let (true, Some(expected_val)) = (FUTEX_CMP_REQUEUE_TEST_YIELD, expected_val) {
            while read_futex_value(uaddr)? == expected_val {
                crate::perf::futex_yield(1);
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
            // 固定锁顺序：FUTEX_QUEUES → MemorySet（读）→ FUTEX_WAITS。
            // 页面已在上方解析；这里的定长无缺页读取只翻译现有 PTE，
            // 持有队列锁期间不会分配内存，也不会阻塞等待缺页处理。
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
