//! #### 任务调度队列
//!
//! 调度器维护按调度策略和优先级分层的就绪队列，并在时钟中断、
//! 主动让出、阻塞或退出时选择下一个任务。
//! 当前架构层 `__switch` 接收下一个任务的内核栈指针，因此这里会完成最后一步切换。

use super::processor::{PROCESSOR, current_task};
use super::task::{TaskControlBlock, task_exit, task_exit_by_signal, task_group_exit};
use crate::{arch::task::__switch, mutex::SpinNoIrqLock};
use alloc::{collections::vec_deque::VecDeque, sync::Arc, vec::Vec};
use bitflags::bitflags;
use core::sync::atomic::{Ordering, compiler_fence};
use hashbrown::HashMap;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref SCHEDULER: SpinNoIrqLock<Scheduler> = SpinNoIrqLock::new(Scheduler::new());
    static ref DEAD_TASKS: SpinNoIrqLock<Vec<Arc<TaskControlBlock>>> =
        SpinNoIrqLock::new(Vec::new());
}

const SCHED_FIFO: usize = 1;
const SCHED_RR: usize = 2;
const SCHED_IDLE: usize = 5;

const RT_QUEUE_COUNT: usize = 100;
const NORMAL_QUEUE_COUNT: usize = 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadyQueue {
    Rt(usize),
    Normal(usize),
    Idle,
}

fn ready_queue_for(task: &TaskControlBlock) -> ReadyQueue {
    match task.sched_policy() {
        SCHED_FIFO | SCHED_RR => {
            let prio = task.sched_priority().clamp(1, 99) as usize;
            ReadyQueue::Rt(prio)
        }
        SCHED_IDLE => ReadyQueue::Idle,
        _ => {
            let nice = task.nice().clamp(-20, 19);
            ReadyQueue::Normal((nice + 20) as usize)
        }
    }
}

fn defer_drop_task(task: Arc<TaskControlBlock>) {
    DEAD_TASKS.lock().push(task);
}

fn cleanup_dead_tasks() {
    let dead_tasks = {
        let mut tasks = DEAD_TASKS.lock();
        core::mem::take(&mut *tasks)
    };
    drop(dead_tasks);
}

#[inline(never)]
fn schedule_barrier() {
    compiler_fence(Ordering::SeqCst);
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        core::arch::asm!("dbar 0", options(nostack, preserves_flags));
    }
    compiler_fence(Ordering::SeqCst);
}

/// 添加新任务到就绪队列。
pub fn add_task(task: Arc<TaskControlBlock>) {
    assert!(task.is_ready());
    SCHEDULER.lock().add(task);
}

/// 从就绪队列中取出队首任务。
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    SCHEDULER.lock().fetch()
}

/// 任务调度属性变化后，若它已经在就绪队列中，则按新策略重新入队。
pub fn requeue_ready_task(task: Arc<TaskControlBlock>) {
    if !task.is_ready() {
        return;
    }
    let mut scheduler = SCHEDULER.lock();
    scheduler.remove(task.tid());
    scheduler.add(task);
}

/// 阻塞任务。
pub fn block_task(task: Arc<TaskControlBlock>) {
    assert!(task.is_blocked());
    SCHEDULER.lock().block(task);
}

pub fn wakeup_stopped_task(task: Arc<TaskControlBlock>) {
    if task.is_stopped() {
        task.set_ready();
        SCHEDULER.lock().add(task);
    }
}

/// 将当前任务标记为阻塞并加入阻塞队列，但暂不切换。
///
/// 返回 `false` 表示当前没有可运行任务，调用者不应让当前任务睡眠。
pub fn prepare_current_task_blocked() -> bool {
    let Some(task) = current_task() else {
        return false;
    };

    let mut scheduler = SCHEDULER.lock();
    if scheduler.is_ready_empty() {
        return false;
    }
    task.set_blocked();
    scheduler.block(task);
    true
}

/// 从就绪队列中移除任务。
pub fn remove_task(tid: usize) {
    SCHEDULER.lock().remove(tid);
}

/// 从就绪队列中移除线程组。
pub fn remove_thread_group(tgid: usize) {
    SCHEDULER.lock().remove_thread_group(tgid);
}

/// 直接调度下一个任务。
///
/// 调用者需要在调用前处理好当前任务的退出或状态变化。
#[unsafe(no_mangle)]
#[inline(never)]
pub fn switch_to_next_task() {
    let Some(current) = current_task() else {
        crate::arch::idle();
    };

    if let Some(next_task) = fetch_task() {
        if Arc::ptr_eq(&current, &next_task) {
            current.set_running();
            cleanup_dead_tasks();
            return;
        }
        let next_task_kernel_stack = next_task.kstack();
        let current_task_ptr = Arc::as_ptr(&current) as usize;
        next_task.set_running();
        PROCESSOR.lock().switch_to(next_task);
        schedule_barrier();
        unsafe {
            __switch(next_task_kernel_stack, current_task_ptr);
        }
        schedule_barrier();
        cleanup_dead_tasks();
        return;
    }

    crate::arch::idle();
}

fn switch_to_task(current: Arc<TaskControlBlock>, next_task: Arc<TaskControlBlock>) {
    if Arc::ptr_eq(&current, &next_task) {
        current.set_running();
        return;
    }

    let current_task_ptr = Arc::as_ptr(&current) as usize;
    let next_task_kernel_stack = next_task.kstack();
    next_task.set_running();
    PROCESSOR.lock().switch_to(next_task);
    schedule_barrier();
    unsafe {
        __switch(next_task_kernel_stack, current_task_ptr);
    }
    schedule_barrier();
    cleanup_dead_tasks();
}

/// 主动让出当前任务。
///
/// 先取下一个任务，再把当前任务放回就绪队列。这样用于轮询式等待时，
/// 当前任务不会立刻凭借较高优先级抢回 CPU。
#[unsafe(no_mangle)]
#[inline(never)]
pub fn yield_current_task() {
    let Some(task) = current_task() else {
        return;
    };

    let mut next_task = fetch_task();
    if next_task.is_none() {
        crate::syscall::check_all_task_timers();
        next_task = fetch_task();
    }

    if let Some(next_task) = next_task {
        if Arc::ptr_eq(&task, &next_task) {
            task.set_running();
            return;
        }
        task.set_ready();
        add_task(task.clone());
        switch_to_task(task, next_task);
    }
}

/// 时间片抢占当前任务。
///
/// 时钟中断触发时先把当前任务放回所属优先级队列队尾，再选择下一个任务。
/// 同一优先级内这会形成简单的 round-robin；不同优先级仍按 RT/nice/idle
/// 的固定顺序调度。
#[unsafe(no_mangle)]
#[inline(never)]
pub fn preempt_current_task() {
    let Some(task) = current_task() else {
        return;
    };

    task.set_ready();
    add_task(task.clone());
    if let Some(next_task) = fetch_task() {
        switch_to_task(task, next_task);
    }
}

/// 阻塞当前任务并运行下一个任务。
#[unsafe(no_mangle)]
#[inline(never)]
pub fn blocking_and_run_next() {
    let Some(task) = current_task() else {
        return;
    };

    if let Some(next_task) = fetch_task() {
        let current_task_ptr = Arc::as_ptr(&task) as usize;
        task.set_blocked();
        block_task(task);

        let next_task_kernel_stack = next_task.kstack();
        next_task.set_running();
        PROCESSOR.lock().switch_to(next_task);
        schedule_barrier();
        unsafe {
            __switch(next_task_kernel_stack, current_task_ptr);
        }
        schedule_barrier();
        cleanup_dead_tasks();
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub fn stop_current_and_run_next() {
    let Some(task) = current_task() else {
        return;
    };

    if let Some(next_task) = fetch_task() {
        let current_task_ptr = Arc::as_ptr(&task) as usize;
        task.set_stopped();

        let next_task_kernel_stack = next_task.kstack();
        next_task.set_running();
        PROCESSOR.lock().switch_to(next_task);
        schedule_barrier();
        unsafe {
            __switch(next_task_kernel_stack, current_task_ptr);
        }
        schedule_barrier();
        cleanup_dead_tasks();
    }
}

#[inline(never)]
fn switch_to_next_task_after_exit() -> ! {
    let Some(current) = current_task() else {
        panic!("Unreachable!");
    };

    loop {
        if let Some(next_task) = fetch_task() {
            let next_task_kernel_stack = next_task.kstack();
            let current_task_ptr = Arc::as_ptr(&current) as usize;
            next_task.set_running();
            PROCESSOR.lock().switch_to(next_task);
            defer_drop_task(current);
            schedule_barrier();
            unsafe {
                __switch(next_task_kernel_stack, current_task_ptr);
            }
            unreachable!("returned to an exited task");
        }

        crate::syscall::check_all_task_timers();
        core::hint::spin_loop();
    }
}

#[unsafe(no_mangle)]
pub fn exit_and_run_next(exit_code: i32) -> ! {
    let Some(task) = current_task() else {
        crate::arch::idle();
    };
    task_exit(task, exit_code);
    switch_to_next_task_after_exit();
}

#[unsafe(no_mangle)]
pub fn exit_by_signal_and_run_next(signal: i32) -> ! {
    let Some(task) = current_task() else {
        crate::arch::idle();
    };
    task_exit_by_signal(task, signal);
    switch_to_next_task_after_exit();
}

#[unsafe(no_mangle)]
pub fn exit_group_and_run_next(exit_code: i32) -> ! {
    let Some(task) = current_task() else {
        crate::arch::idle();
    };
    task_group_exit(task, exit_code);
    switch_to_next_task_after_exit();
}

/// 简单时间片轮转调度器。
///
/// - RT 队列：`SCHED_FIFO/SCHED_RR`，优先级 1..99，数值越大越先运行；
/// - 普通队列：`SCHED_OTHER/BATCH`，按 nice -20..19 分 40 档，nice 越小越先运行；
/// - Idle 队列：`SCHED_IDLE`，仅在没有 RT/普通任务时运行；
/// - 同一优先级内使用 `push_back` + `pop_front`，时钟中断触发的 `preempt_current_task`
///   会把当前任务放回队尾，从而形成简单 RR。
pub struct Scheduler {
    rt_queues: Vec<VecDeque<Arc<TaskControlBlock>>>,
    normal_queues: Vec<VecDeque<Arc<TaskControlBlock>>>,
    idle_queue: VecDeque<Arc<TaskControlBlock>>,
    rt_bitmap: u128,
    normal_bitmap: u64,
    task_index: HashMap<usize, ReadyQueue>,
    blocked_tasks: HashMap<usize, Arc<TaskControlBlock>>,
}

impl Scheduler {
    /// 创建一个空调度器。
    pub fn new() -> Self {
        Self {
            rt_queues: (0..RT_QUEUE_COUNT).map(|_| VecDeque::new()).collect(),
            normal_queues: (0..NORMAL_QUEUE_COUNT).map(|_| VecDeque::new()).collect(),
            idle_queue: VecDeque::new(),
            rt_bitmap: 0,
            normal_bitmap: 0,
            task_index: HashMap::new(),
            blocked_tasks: HashMap::new(),
        }
    }

    /// 添加任务到调度器就绪队列。
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        let tid = task.tid();
        let queue = ready_queue_for(&task);
        if let Some(old_queue) = self.task_index.insert(tid, queue) {
            debug_assert!(false, "task {tid} is already queued");
            self.remove_from_ready_queue(tid, old_queue);
        }
        match queue {
            ReadyQueue::Rt(idx) => {
                self.rt_queues[idx].push_back(task);
                self.rt_bitmap |= 1u128 << (idx - 1);
            }
            ReadyQueue::Normal(idx) => {
                self.normal_queues[idx].push_back(task);
                self.normal_bitmap |= 1u64 << idx;
            }
            ReadyQueue::Idle => self.idle_queue.push_back(task),
        }
    }

    /// 取出最高优先级队列中的队首任务。
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        while self.rt_bitmap != 0 {
            let bit = 127 - self.rt_bitmap.leading_zeros() as usize;
            let idx = bit + 1;
            if idx < RT_QUEUE_COUNT {
                if let Some(task) = self.rt_queues[idx].pop_front() {
                    self.task_index.remove(&task.tid());
                    if self.rt_queues[idx].is_empty() {
                        self.rt_bitmap &= !(1u128 << bit);
                    }
                    return Some(task);
                }
            }
            self.rt_bitmap &= !(1u128 << bit);
        }

        while self.normal_bitmap != 0 {
            let idx = self.normal_bitmap.trailing_zeros() as usize;
            if idx < NORMAL_QUEUE_COUNT {
                if let Some(task) = self.normal_queues[idx].pop_front() {
                    self.task_index.remove(&task.tid());
                    if self.normal_queues[idx].is_empty() {
                        self.normal_bitmap &= !(1u64 << idx);
                    }
                    return Some(task);
                }
            }
            self.normal_bitmap &= !(1u64 << idx);
        }

        let task = self.idle_queue.pop_front()?;
        self.task_index.remove(&task.tid());
        Some(task)
    }

    /// 是否没有可运行任务。
    pub fn is_ready_empty(&self) -> bool {
        self.rt_bitmap == 0 && self.normal_bitmap == 0 && self.idle_queue.is_empty()
    }

    /// 从调度器就绪队列中移除任务。
    pub fn remove(&mut self, tid: usize) {
        if let Some(queue) = self.task_index.remove(&tid) {
            self.remove_from_ready_queue(tid, queue);
        }
        self.blocked_tasks.remove(&tid);
    }

    /// 从调度器就绪队列中移除线程组。
    pub fn remove_thread_group(&mut self, tgid: usize) {
        let mut removed = Vec::new();
        for idx in 1..RT_QUEUE_COUNT {
            self.rt_queues[idx].retain(|task| {
                if task.tgid() == tgid {
                    removed.push(task.tid());
                    false
                } else {
                    true
                }
            });
            if self.rt_queues[idx].is_empty() {
                self.rt_bitmap &= !(1u128 << (idx - 1));
            }
        }
        for idx in 0..NORMAL_QUEUE_COUNT {
            self.normal_queues[idx].retain(|task| {
                if task.tgid() == tgid {
                    removed.push(task.tid());
                    false
                } else {
                    true
                }
            });
            if self.normal_queues[idx].is_empty() {
                self.normal_bitmap &= !(1u64 << idx);
            }
        }
        self.idle_queue.retain(|task| {
            if task.tgid() == tgid {
                removed.push(task.tid());
                false
            } else {
                true
            }
        });
        for tid in removed {
            self.task_index.remove(&tid);
        }
        self.blocked_tasks.retain(|_, task| task.tgid() != tgid);
    }

    /// 阻塞任务。
    pub fn block(&mut self, task: Arc<TaskControlBlock>) {
        let tid = task.tid();
        debug_assert!(
            !self.blocked_tasks.contains_key(&tid),
            "task {tid} is already blocked"
        );
        self.blocked_tasks.insert(tid, task);
    }

    /// 从阻塞队列中移除指定 tid 的任务。
    pub fn wake(&mut self, tid: usize) -> Option<Arc<TaskControlBlock>> {
        self.blocked_tasks.remove(&tid)
    }

    fn remove_from_ready_queue(&mut self, tid: usize, queue: ReadyQueue) {
        match queue {
            ReadyQueue::Rt(idx) => {
                self.rt_queues[idx].retain(|task| task.tid() != tid);
                if self.rt_queues[idx].is_empty() {
                    self.rt_bitmap &= !(1u128 << (idx - 1));
                }
            }
            ReadyQueue::Normal(idx) => {
                self.normal_queues[idx].retain(|task| task.tid() != tid);
                if self.normal_queues[idx].is_empty() {
                    self.normal_bitmap &= !(1u64 << idx);
                }
            }
            ReadyQueue::Idle => {
                self.idle_queue.retain(|task| task.tid() != tid);
            }
        }
    }
}

/// 唤醒指定 tid 的任务，将其从 blocked_queue 移入 ready_queue。
pub fn wakeup_task(tid: usize) {
    let mut scheduler = SCHEDULER.lock();
    if let Some(task) = scheduler.wake(tid) {
        task.set_ready();
        scheduler.add(task);
    }
}

bitflags! {
    pub struct WaitOption: i32 {
        /// 这个选项用于非阻塞挂起。当与 wait 或 waitpid 一起使用时，如果没有任何子进程状态改变，
        /// 这些系统调用不会阻塞父进程，而是立即返回。在 Linux 中，如果没有子进程处于可等待的状态，wait 或 waitpid 会返回 0。
        const WNOHANG = 1;
        /// 这个选项告诉 wait 或 waitpid 也报告那些已经停止（stopped），但尚未终止的子进程的状态。默认情况下，
        /// 只有当子进程终止时，它们的结束状态才会被报告。如果子进程被某种信号（如 SIGSTOP 或 SIGTSTP）停止，
        /// 并且父进程没有设置 WUNTRACED 选项，那么父进程将不会感知到子进程的停止状态，直到子进程被继续执行或终止。
        const WUNTRACED = 1 << 1;
        /// 当子进程被停止后又继续执行时，使用这个选项。如果子进程之前被一个停止信号（如SIGSTOP 或 SIGTSTP）暂停，
        /// 然后通过继续信号（如 SIGCONT）被继续执行，那么 wait 或 waitpid 将报告这个子进程的状态，
        /// 即使它还没有终止。这允许父进程知道子进程已经从停止状态恢复。
        const WCONTINUED = 1 << 3;
    }
}
