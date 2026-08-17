// os/src/task/processor.rs

//! #### 任务调度之 CPU 状态转换
//!
//! - 功能：processor 可以依据调度策略切换任务上下文转而执行目标任务
//!
//! - 理解：上下文切换的关键是 [`__switch`] 函数，该函数保存和恢复

use super::scheduler::{cleanup_dead_tasks, fetch_task};
use super::task::TaskControlBlock;
use crate::arch::task::__switch;
use crate::mutex::SpinNoIrqLock;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

const MAX_CPUS: usize = crate::arch::smp::MAX_HARTS;

// 每个 CPU 都需要独立的 bootstrap/idle context。它们不进入 task manager，
// 仅用于保存首次切入用户任务前的内核上下文。
lazy_static! {
    static ref IDLE_TASKS: [Arc<TaskControlBlock>; MAX_CPUS] =
        core::array::from_fn(|_| Arc::new(TaskControlBlock::zero_init()));
}

/// 只能由 boot hart 在启动 secondary 之前调用，避免多个 secondary 首次
/// 进入 idle loop 时并发执行 lazy_static 的 Arc/KernelStack 初始化。
pub fn init_per_cpu_idle_tasks() {
    lazy_static::initialize(&IDLE_TASKS);
}

static PROCESSORS: [SpinNoIrqLock<Processor>; MAX_CPUS] =
    [const { SpinNoIrqLock::new(Processor::new()) }; MAX_CPUS];

#[repr(align(64))]
struct PerHartIdleTicks(AtomicUsize);

static IDLE_TICKS: [PerHartIdleTicks; MAX_CPUS] =
    [const { PerHartIdleTicks(AtomicUsize::new(0)) }; MAX_CPUS];

#[inline]
fn account_idle_ticks(ticks: usize) {
    IDLE_TICKS[current_cpu_id()]
        .0
        .fetch_add(ticks, Ordering::Relaxed);
}

/// Return aggregate idle time across all harts, matching `/proc/uptime`'s
/// second-field convention.  The current sub-tick idle interval is accounted
/// when `wait_for_interrupt()` returns, so a read may lag by at most one timer
/// period per idle hart while remaining monotonic.
pub fn system_idle_time_us() -> usize {
    let ticks = IDLE_TICKS.iter().fold(0usize, |total, idle| {
        total.saturating_add(idle.0.load(Ordering::Relaxed))
    });
    let freq = crate::arch::timer::get_hardware_clock_freq();
    ticks / freq * 1_000_000 + ticks % freq * 1_000_000 / freq
}

/// 处理器管理
///
/// 管理维护 CPU 状态
pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    /// A running task that has already switched to this CPU's idle context,
    /// but has not yet been published to the global ready queue.  Keeping the
    /// Arc here makes the context-save → ready-publication handoff explicit.
    handoff: Option<TaskHandoff>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContextSwitchKind {
    None,
    Voluntary,
    Involuntary,
}

struct TaskHandoff {
    task: Arc<TaskControlBlock>,
    switch_kind: ContextSwitchKind,
}

#[inline]
pub fn current_processor() -> &'static SpinNoIrqLock<Processor> {
    &PROCESSORS[crate::arch::smp::current_hart_id()]
}

#[inline]
fn current_cpu_id() -> usize {
    crate::arch::smp::current_hart_id()
}

#[inline]
fn current_idle_task() -> Arc<TaskControlBlock> {
    IDLE_TASKS[crate::arch::smp::current_hart_id()].clone()
}

impl Processor {
    pub const fn new() -> Self {
        Self {
            current: None,
            handoff: None,
        }
    }

    /// 取出当前执行的任务
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }

    /// 返回当前执行的任务的一份拷贝
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }

    /// 切换当前 CPU 记录的运行任务。
    pub fn switch_to(&mut self, task: Arc<TaskControlBlock>) {
        assert!(
            self.current.is_none(),
            "switching with an existing current task"
        );
        self.current = Some(task);
    }

    fn handoff_current(&mut self, current: &Arc<TaskControlBlock>, switch_kind: ContextSwitchKind) {
        let running = self.current.take().expect("handoff without current task");
        assert!(
            Arc::ptr_eq(&running, current),
            "handoff task differs from this CPU current task"
        );
        assert!(self.handoff.is_none(), "nested task handoff on one CPU");
        self.handoff = Some(TaskHandoff {
            task: running,
            switch_kind,
        });
    }

    fn take_handoff(&mut self) -> Option<TaskHandoff> {
        self.handoff.take()
    }
}

/// 取出当前执行的任务
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    current_processor().lock().take_current()
}

/// 获取当前执行的任务的一份拷贝
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    current_processor().lock().current()
}

/// 获取当前执行的任务的页表基址寄存器值
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    let token = task.get_user_token();
    token
}

/// Yield a running task through this CPU's idle context.
///
/// A multicore scheduler must never add the still-executing task directly to a
/// shared ready queue: another CPU could restore its saved stack before this
/// CPU has finished `__switch`.  Instead `__switch` first saves the task and
/// restores the per-CPU idle context; `run_tasks()` then publishes the saved
/// task from that idle context.
pub(crate) fn handoff_current_to_idle(
    current: Arc<TaskControlBlock>,
    switch_kind: ContextSwitchKind,
) {
    let idle_task = current_idle_task();
    let idle_kstack = idle_task.kstack();
    assert_ne!(idle_kstack, 0, "idle context was not initialized");
    #[cfg(target_arch = "loongarch64")]
    idle_task.set_saved_mmu_token(crate::mm::kernel_mmu_token());
    {
        let mut processor = current_processor().lock();
        processor.handoff_current(&current, switch_kind);
    }
    unsafe {
        __switch(idle_kstack, Arc::as_ptr(&current) as usize);
    }
}

fn publish_saved_handoff() -> Option<TaskHandoff> {
    #[cfg(target_arch = "loongarch64")]
    crate::mm::ensure_kernel_space_active();
    let handoff = current_processor().lock().take_handoff();
    if let Some(handoff) = handoff.as_ref() {
        let task = &handoff.task;
        // __switch has restored this CPU's idle context and kernel satp before
        // returning here, so the outgoing address space is no longer active.
        task.clear_memory_set_current_hart_active();
        let was_running = task.is_running() && !task.termination_requested();
        if was_running {
            // A yielding/preempted task is not visible to any other CPU until
            // its context has reached this idle loop.
            task.set_ready();
            super::scheduler::add_task_before_owner_release(task.clone());
        }
        task.release_cpu_owner(current_cpu_id());
        crate::arch::smp::kick_one_idle_hart_in(task.cpu_affinity_mask());
    }
    handoff
}

fn account_completed_handoff(handoff: Option<&TaskHandoff>, next: Option<&Arc<TaskControlBlock>>) {
    let Some(handoff) = handoff else {
        return;
    };
    if next.is_some_and(|next| Arc::ptr_eq(next, &handoff.task)) {
        return;
    }
    match handoff.switch_kind {
        ContextSwitchKind::None => {}
        ContextSwitchKind::Voluntary => handoff.task.note_voluntary_context_switch(),
        ContextSwitchKind::Involuntary => handoff.task.note_involuntary_context_switch(),
    }
}

/// 运行任务
///
/// 该函数仅在每 CPU 的 idle context 中运行。
pub fn run_tasks() -> ! {
    loop {
        #[cfg(target_arch = "loongarch64")]
        crate::mm::ensure_kernel_space_active();
        // This executes only after the outgoing task's __switch has saved its
        // context and stopped executing it on this CPU.
        let handoff = publish_saved_handoff();
        // The outgoing context is now on this CPU's idle stack, so deferred
        // TCBs can be dropped even if no user task will ever wake again.
        cleanup_dead_tasks();
        if crate::arch::smp::is_timer_service_hart() {
            crate::fs::ext4::flush_expired_lazytime_inodes_if_needed();
            crate::net::poll_background();
        }
        let mut next_task = fetch_task();
        // Linux charges nvcsw/nivcsw only when scheduling selects a task other
        // than prev.  RespOS always crosses its per-CPU idle stack to publish a
        // saved context safely; selecting the same Arc again is an internal
        // handoff, not an observable task context switch.
        account_completed_handoff(handoff.as_ref(), next_task.as_ref());
        if next_task.is_none() {
            crate::arch::smp::enter_idle();

            // 发布 idle 后再取一次任务。enqueue 若发生在首次 fetch 与
            // enter_idle 之间，不会给我们发 IPI；这次检查避免遗漏它。
            next_task = fetch_task();
            if next_task.is_none() {
                #[cfg(target_arch = "riscv64")]
                crate::arch::trap::enable_timer_interrupt();
                #[cfg(target_arch = "loongarch64")]
                unsafe {
                    // LA kernel code normally runs with IE=0. Idle owns no
                    // locks and may enable interrupts until timer/IPI wakeup.
                    crate::arch::register::crmd::set_interrupt_enabled(true);
                }
                let idle_started = crate::timer::get_time();
                crate::arch::wait_for_interrupt();
                let idle_elapsed = crate::timer::get_time().wrapping_sub(idle_started);
                account_idle_ticks(idle_elapsed);
                crate::perf::idle_ticks(idle_elapsed);
                #[cfg(target_arch = "loongarch64")]
                unsafe {
                    crate::arch::register::crmd::set_interrupt_enabled(false);
                }
            }

            crate::arch::smp::leave_idle();
        }

        if let Some(next_task) = next_task {
            let idle_task = current_idle_task();
            let next_task_kstack = next_task.kstack();
            #[cfg(target_arch = "riscv64")]
            {
                let per_cpu = crate::arch::smp::current_per_cpu_ptr();
                next_task.set_kernel_tp(per_cpu);
                crate::arch::smp::set_current_kernel_sp(next_task.kernel_stack_top_edge());
            }
            next_task.mark_memory_set_current_hart_active();
            let idle_task_ptr = Arc::as_ptr(&idle_task) as usize;
            idle_task.set_ready();
            next_task.assert_cpu_owner(current_cpu_id());
            let mut processor = current_processor().lock();
            processor.switch_to(next_task.clone());
            drop(processor);
            // Keep one Arc on the idle stack across the switch so the exact
            // outgoing task can close its CPU-accounting interval even when
            // it exited and removed itself from the task manager.
            let running_started = crate::timer::get_time();
            next_task.begin_cpu_run(current_cpu_id(), running_started);
            crate::perf::task_running_begin();
            crate::perf::context_switch(1);
            #[cfg(target_arch = "riscv64")]
            crate::perf::local_sfence(1);
            unsafe {
                __switch(next_task_kstack, idle_task_ptr);
            }
            let running_finished = crate::timer::get_time();
            next_task.end_cpu_run(current_cpu_id(), running_finished);
            crate::perf::task_running_end();
            crate::perf::task_running_ticks(running_finished.wrapping_sub(running_started));
            drop(next_task);
            // 任务在无 runnable work 时会恢复本 CPU 的 idle context，继续
            // 从全局 ready queue 选择下一个已 claim 的任务。
            continue;
        }
    }
}
