//! #### 任务调度队列
//!
//! 调度器维护按调度策略和优先级分层的就绪队列，并在时钟中断、
//! 主动让出、阻塞或退出时选择下一个任务。
//! 当前架构层 `__switch` 接收下一个任务的内核栈指针，因此这里会完成最后一步切换。
//!
//! 队列中的任务必须与 TCB 状态一致：同一任务不能重复入队，blocked/zombie 不能被普通
//! fetch 选中，唤醒与 timeout/signal 竞争时只有一个路径能把任务重新置为 ready。实时
//! FIFO/RR、普通优先级和 idle policy 使用不同队列，但 affinity 过滤和 SMP 唤醒规则必须
//! 一致。
//!
//! `DEAD_TASKS` 延迟释放刚切出的任务，防止仍运行在其内核栈上时析构自身。清理只能发生在
//! 已切换到另一栈之后；新增退出快路径时不得绕过这一安全点。

use super::processor::{current_task, handoff_current_to_idle, ContextSwitchKind};
use super::task::{task_exit, task_exit_by_signal, task_group_exit, TaskControlBlock};
use crate::mutex::SpinNoIrqLock;
use alloc::{collections::vec_deque::VecDeque, sync::Arc, vec::Vec};
use bitflags::bitflags;
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

#[inline(always)]
fn lock_scheduler() -> crate::mutex::MutexGuard<'static, Scheduler, crate::mutex::NoIrqLock> {
    let started = crate::perf::now_ticks();
    let guard = SCHEDULER.lock();
    crate::perf::observe_scheduler_lock_wait(crate::perf::elapsed_since(started));
    guard
}

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

pub(crate) fn cleanup_dead_tasks() {
    let dead_tasks = {
        let mut tasks = DEAD_TASKS.lock();
        core::mem::take(&mut *tasks)
    };
    drop(dead_tasks);
}

/// 添加新任务到就绪队列。
pub fn add_task(task: Arc<TaskControlBlock>) {
    assert!(task.is_ready());
    let affinity = task.cpu_affinity_mask();
    lock_scheduler().add(task);
    crate::arch::smp::kick_one_idle_hart_in(affinity);
}

/// 发布一个保存上下文仍由原 CPU 持有的任务。
/// 调用者随后立即释放所有权，并且仅在交接完成后发送一次必要的核间唤醒。
pub(crate) fn add_task_before_owner_release(task: Arc<TaskControlBlock>) {
    assert!(task.is_ready());
    lock_scheduler().add(task);
}

/// 从就绪队列中取出队首任务，并在同一把 scheduler 锁内将其 claim 为
/// `Running`。
///
/// 多核下不能先出队、释放锁、再由调用者改 task status；否则 wakeup/signal
/// 可以观察到任务既不在 ready queue、又仍为 `Ready` 的窗口。该任务的实际
/// context switch 仍在锁外完成，避免把 scheduler 锁带入 `__switch`。
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    let mut scheduler = lock_scheduler();
    let cpu = crate::arch::smp::current_hart_id();
    let task = scheduler.fetch_for_cpu(cpu);
    if let Some(task) = task {
        debug_assert!(task.is_ready(), "claimed task {} is not Ready", task.tid());
        if task.try_claim_running_on_cpu(cpu) {
            return Some(task);
        }
        // 唤醒者可能在原 CPU 尚未保存完上下文时就发布了本任务。
        // 保留 Ready 状态，但必须等原 CPU 在 idle 上下文中释放所有权，
        // 其他 CPU 才能恢复该上下文。
        scheduler.add(task);
        return None;
    }
    None
}

/// 任务调度属性变化后，若它已经在就绪队列中，则按新策略重新入队。
pub fn requeue_ready_task(task: Arc<TaskControlBlock>) {
    if !task.is_ready() {
        return;
    }
    let mut scheduler = lock_scheduler();
    scheduler.remove(task.tid());
    let affinity = task.cpu_affinity_mask();
    scheduler.add(task);
    drop(scheduler);
    crate::arch::smp::kick_one_idle_hart_in(affinity);
}

/// 阻塞任务。
pub fn block_task(task: Arc<TaskControlBlock>) {
    assert!(task.is_blocked());
    lock_scheduler().block(task);
}

pub fn wakeup_stopped_task(task: Arc<TaskControlBlock>) {
    if task.is_stopped() {
        task.set_ready();
        let affinity = task.cpu_affinity_mask();
        lock_scheduler().add(task);
        crate::arch::smp::kick_one_idle_hart_in(affinity);
    }
}

/// 将当前任务标记为阻塞并加入阻塞索引，但暂不执行上下文切换。
///
/// 等待原语应先发布自身 waiter，再调用本函数，并在切换前重新检查完成条件，
/// 以关闭“事件发生在条件检查与 Blocked 可见之间”的丢失唤醒窗口。
/// 返回 `false` 仅表示当前 CPU 没有用户任务；即使就绪队列暂时为空也允许阻塞，
/// 因为定时器、中断或另一 CPU 可以稍后唤醒任务。
pub fn prepare_current_task_blocked() -> bool {
    let Some(task) = current_task() else {
        return false;
    };

    let mut scheduler = lock_scheduler();
    task.set_blocked();
    scheduler.block(task);
    true
}

/// 从就绪队列中移除任务。
pub fn remove_task(tid: usize) {
    lock_scheduler().remove(tid);
}

/// 从就绪队列中移除线程组。
pub fn remove_thread_group(tgid: usize) {
    lock_scheduler().remove_thread_group(tgid);
}

/// 将已经变为 Blocked/Stopped/Exited 的当前任务交给 idle 上下文，等待真正唤醒后恢复。
///
/// 调用者必须在进入前完成状态发布和等待队列登记。本函数不自行选择下一个任务，
/// 而是通过每 CPU idle 栈完成上下文保存；若唤醒恰好在切换前发生，循环会看到 Running
/// 并返回原系统调用，否则直到调度器重新认领本任务才会继续。
#[unsafe(no_mangle)]
#[inline(never)]
pub fn switch_to_next_task() {
    let Some(current) = current_task() else {
        crate::arch::idle();
    };
    crate::perf::blocking_switch(1);
    loop {
        // 定时器中断可能已从这个阻塞上下文切走；真正的唤醒再次调度它时，
        // 应从原先发生阻塞的系统调用处继续执行。
        if current.is_running() {
            cleanup_dead_tasks();
            return;
        }

        // 阻塞任务可能在当前 CPU 进入 idle 之前就被另一 CPU 唤醒。
        // 当前 CPU 保存完上下文前仍保留所有权；在此之前 fetch_task()
        // 会让这个刚变为 Ready 的任务继续留在队列中。
        handoff_current_to_idle(current.clone(), ContextSwitchKind::Voluntary);
    }
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
    crate::perf::scheduler_yield(1);
    handoff_current_to_idle(task, ContextSwitchKind::Voluntary);
}

/// 在定时器安全点抢占当前任务，并经 idle 上下文重新参与调度。
///
/// 时钟中断触发时先把当前任务放回所属优先级队列队尾，再选择下一个任务。
/// 同一优先级内这会形成简单的 round-robin；不同优先级仍按 RT/nice/idle
/// 的固定顺序调度。若定时器打断的是正在执行阻塞/退出交接的内核路径，不能把它
/// 错误恢复为 Ready，此时只完成上下文保存并等待权威状态的真正唤醒。
#[unsafe(no_mangle)]
#[inline(never)]
pub fn preempt_current_task() {
    let Some(task) = current_task() else {
        return;
    };
    crate::perf::timer_preemption(1);

    // switch_to_next_task 等待第一个可运行任务时也可能收到定时器中断。
    // 不得把当前这个已阻塞、停止或退出的上下文重新标成 Ready，只调度真正被唤醒的任务。
    // 若被唤醒的恰好是当前任务，下方取出它时会恢复 Running，
    // 被中断的调度循环随后返回原调用者。
    let switch_kind = if task.is_running() {
        ContextSwitchKind::Involuntary
    } else {
        ContextSwitchKind::None
    };
    handoff_current_to_idle(task, switch_kind);
}

/// 阻塞当前任务并运行下一个任务。
#[unsafe(no_mangle)]
#[inline(never)]
pub fn blocking_and_run_next() {
    let Some(task) = current_task() else {
        return;
    };

    task.set_blocked();
    block_task(task);
    switch_to_next_task();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub fn stop_current_and_run_next() {
    let Some(task) = current_task() else {
        return;
    };

    // 信号路径先发布 Stopped 再通知父进程。父进程可能已用 SIGCONT 将其改为 Ready，
    // 因此这里不能覆盖并发唤醒，只需完成当前上下文的保存。
    handoff_current_to_idle(task, ContextSwitchKind::Voluntary);
    cleanup_dead_tasks();
}

#[inline(never)]
fn switch_to_next_task_after_exit() -> ! {
    let Some(current) = current_task() else {
        panic!("Unreachable!");
    };

    loop {
        defer_drop_task(current.clone());
        handoff_current_to_idle(current, ContextSwitchKind::None);
        unreachable!("returned to exited task");
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
        crate::perf::observe_scheduler_ready(self.task_index.len());
        self.debug_assert_invariants();
    }

    /// 取出 `cpu` 允许运行的最高优先级任务。
    ///
    /// 不符合亲和性的任务仍留在原位：CPU 必须能跳过队首的绑核任务，
    /// 转而执行本 CPU 可运行的较低优先级任务，同时不破坏绑核任务所在 CPU 的 FIFO 顺序。
    pub fn fetch_for_cpu(&mut self, cpu: usize) -> Option<Arc<TaskControlBlock>> {
        let mut rt_candidates = self.rt_bitmap;
        while rt_candidates != 0 {
            let bit = 127 - rt_candidates.leading_zeros() as usize;
            rt_candidates &= !(1u128 << bit);
            let idx = bit + 1;
            if idx < RT_QUEUE_COUNT {
                if let Some(pos) = self.rt_queues[idx]
                    .iter()
                    .position(|task| task.can_be_claimed_on_cpu(cpu))
                {
                    let task = self.rt_queues[idx]
                        .remove(pos)
                        .expect("ready task position disappeared");
                    self.task_index.remove(&task.tid());
                    crate::perf::observe_scheduler_ready(self.task_index.len());
                    if self.rt_queues[idx].is_empty() {
                        self.rt_bitmap &= !(1u128 << bit);
                    }
                    self.debug_assert_invariants();
                    return Some(task);
                }
            }
        }

        let mut normal_candidates = self.normal_bitmap;
        while normal_candidates != 0 {
            let idx = normal_candidates.trailing_zeros() as usize;
            normal_candidates &= !(1u64 << idx);
            if idx < NORMAL_QUEUE_COUNT {
                if let Some(pos) = self.normal_queues[idx]
                    .iter()
                    .position(|task| task.can_be_claimed_on_cpu(cpu))
                {
                    let task = self.normal_queues[idx]
                        .remove(pos)
                        .expect("ready task position disappeared");
                    self.task_index.remove(&task.tid());
                    crate::perf::observe_scheduler_ready(self.task_index.len());
                    if self.normal_queues[idx].is_empty() {
                        self.normal_bitmap &= !(1u64 << idx);
                    }
                    self.debug_assert_invariants();
                    return Some(task);
                }
            }
        }

        let task = self
            .idle_queue
            .iter()
            .position(|task| task.can_be_claimed_on_cpu(cpu))
            .and_then(|pos| self.idle_queue.remove(pos));
        if let Some(task) = &task {
            self.task_index.remove(&task.tid());
            crate::perf::observe_scheduler_ready(self.task_index.len());
        }
        self.debug_assert_invariants();
        task
    }

    /// 从调度器就绪队列中移除任务。
    pub fn remove(&mut self, tid: usize) {
        if let Some(queue) = self.task_index.remove(&tid) {
            self.remove_from_ready_queue(tid, queue);
            crate::perf::observe_scheduler_ready(self.task_index.len());
        }
        self.blocked_tasks.remove(&tid);
        self.debug_assert_invariants();
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
        crate::perf::observe_scheduler_ready(self.task_index.len());
        self.blocked_tasks.retain(|_, task| task.tgid() != tgid);
        self.debug_assert_invariants();
    }

    /// 阻塞任务。
    pub fn block(&mut self, task: Arc<TaskControlBlock>) {
        let tid = task.tid();
        debug_assert!(
            !self.blocked_tasks.contains_key(&tid),
            "task {tid} is already blocked"
        );
        self.blocked_tasks.insert(tid, task);
        self.debug_assert_invariants();
    }

    /// 从阻塞队列中移除指定 tid 的任务。
    pub fn wake(&mut self, tid: usize) -> Option<Arc<TaskControlBlock>> {
        let task = self.blocked_tasks.remove(&tid);
        self.debug_assert_invariants();
        task
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

    #[inline(always)]
    fn debug_assert_invariants(&self) {
        #[cfg(debug_assertions)]
        self.assert_invariants();
    }

    #[cfg(debug_assertions)]
    /// 完整核对 ready queues、blocked 索引和 task_index 的双向一致性。
    ///
    /// 每个 tid 必须只出现一次，队列分类要匹配任务 status/调度属性，Ready 任务不能同时在
    /// blocked 集合。该检查只在调试门禁调用；持 SpinNoIrqLock 时避免额外的笛卡尔扫描，
    /// 防止启动核延误唯一 timer service。
    fn assert_invariants(&self) {
        assert!(
            self.rt_queues[0].is_empty(),
            "unused RT queue 0 must remain empty"
        );

        let mut ready_count = 0usize;
        let mut expected_rt_bitmap = 0u128;
        for idx in 1..RT_QUEUE_COUNT {
            let queue = &self.rt_queues[idx];
            if !queue.is_empty() {
                expected_rt_bitmap |= 1u128 << (idx - 1);
            }
            for task in queue {
                self.assert_ready_task(task, ReadyQueue::Rt(idx));
                ready_count += 1;
            }
        }
        assert_eq!(
            self.rt_bitmap, expected_rt_bitmap,
            "RT bitmap does not match non-empty queues"
        );

        let mut expected_normal_bitmap = 0u64;
        for idx in 0..NORMAL_QUEUE_COUNT {
            let queue = &self.normal_queues[idx];
            if !queue.is_empty() {
                expected_normal_bitmap |= 1u64 << idx;
            }
            for task in queue {
                self.assert_ready_task(task, ReadyQueue::Normal(idx));
                ready_count += 1;
            }
        }
        assert_eq!(
            self.normal_bitmap, expected_normal_bitmap,
            "normal bitmap does not match non-empty queues"
        );

        for task in &self.idle_queue {
            self.assert_ready_task(task, ReadyQueue::Idle);
            ready_count += 1;
        }

        assert_eq!(
            self.task_index.len(),
            ready_count,
            "task_index and ready queues have different sizes"
        );
        // 上面已经逐项核对队列与 task_index；add() 会拒绝重复 tid，
        // 因而两边基数相等也能证明索引中不存在未进入就绪队列的多余项。
        // 持有 SpinNoIrqLock 时不要再次做 task_index × 全队列扫描，
        // 否则在启动核上可能延误唯一的全局超时服务。

        for (tid, task) in &self.blocked_tasks {
            assert_eq!(*tid, task.tid(), "blocked_tasks key/tid mismatch");
            assert!(task.is_blocked(), "blocked task {tid} is not Blocked");
            assert!(
                !self.task_index.contains_key(tid),
                "task {tid} appears in both ready and blocked sets"
            );
            assert!(!task.is_exited(), "exited task {tid} remains blocked");
        }
    }

    #[cfg(debug_assertions)]
    fn assert_ready_task(&self, task: &TaskControlBlock, queue: ReadyQueue) {
        let tid = task.tid();
        assert!(task.is_ready(), "ready-queued task {tid} is not Ready");
        assert!(!task.is_exited(), "exited task {tid} remains ready");
        assert_eq!(
            ready_queue_for(task),
            queue,
            "task {tid} is in the wrong ready queue"
        );
        assert_eq!(
            self.task_index.get(&tid),
            Some(&queue),
            "ready task {tid} is missing or misindexed"
        );
        assert!(
            !self.blocked_tasks.contains_key(&tid),
            "task {tid} appears in both ready and blocked sets"
        );
    }
}

/// 唤醒指定 tid 的任务，将其从 blocked_queue 移入 ready_queue。
pub fn wakeup_task(tid: usize) {
    let mut scheduler = lock_scheduler();
    let affinity = if let Some(task) = scheduler.wake(tid) {
        task.set_ready();
        let affinity = task.cpu_affinity_mask();
        scheduler.add(task);
        Some(affinity)
    } else {
        None
    };
    drop(scheduler);
    if let Some(affinity) = affinity {
        crate::arch::smp::kick_one_idle_hart_in(affinity);
    }
}

pub fn scheduler_health_counts() -> Option<(usize, usize, usize)> {
    let (ready, blocked) = {
        let scheduler = SCHEDULER.try_lock()?;
        (scheduler.task_index.len(), scheduler.blocked_tasks.len())
    };
    let deferred = DEAD_TASKS.try_lock()?.len();
    Some((ready, blocked, deferred))
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
