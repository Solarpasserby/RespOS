// os/src/task/processor.rs

//! #### 任务调度之 CPU 状态转换
//!
//! processor 维护每个 hart 当前任务和 idle/bootstrap context，是 scheduler 的队列状态
//! 与架构 `__switch` 汇编之间的交接层。scheduler 选择任务后，这里负责安装 current、
//! 切换地址空间/内核栈并进行 accounting；返回时旧任务可能已经退出，不能再依赖裸引用。
//!
//! 上下文切换的关键是 [`__switch`]：它只保存内核续执行所需的 callee-saved 状态，用户
//! 寄存器由 trap context 管理。持有 scheduler 或 task 自旋锁跨越 `__switch` 会把锁所有权
//! 交给另一条执行流，除非显式采用并证明 handoff 协议，否则禁止这样做。

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

/// 返回所有硬件线程的累计空闲时间，语义与 `/proc/uptime` 第二个字段一致。
/// 当前不足一个 tick 的空闲区间会在 `wait_for_interrupt()` 返回时记账，
/// 因而读取值保持单调，但每个空闲核最多落后一个定时周期。
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
    /// 已切换到本 CPU idle 上下文、但尚未发布到全局就绪队列的运行任务。
    /// 在这里保留 Arc，使“保存上下文 → 发布 Ready”的所有权交接显式可见。
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

/// 让运行任务经由本 CPU 的 idle 上下文主动让出处理器。
///
/// 多核调度器绝不能把仍在执行的任务直接加入共享就绪队列：当前 CPU 尚未完成
/// `__switch` 时，另一 CPU 就可能恢复其栈。正确顺序是 `__switch` 先保存任务并恢复
/// 每 CPU idle 上下文，再由 `run_tasks()` 从 idle 上下文发布已保存的任务。
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

/// 在 idle 栈上发布刚刚完成保存的换出任务，并释放其 CPU 上下文所有权。
///
/// 只有执行到本函数时才能证明 `__switch` 已经停止使用任务内核栈。仍需继续运行的任务
/// 先以 Ready 状态加入全局队列，再释放 `cpu_owner`；这一顺序使其他 CPU 即使看见队列项，
/// 也必须等所有权释放后才能恢复它。退出或收到终止请求的任务不会重新入队。
fn publish_saved_handoff() -> Option<TaskHandoff> {
    #[cfg(target_arch = "loongarch64")]
    crate::mm::ensure_kernel_space_active();
    let handoff = current_processor().lock().take_handoff();
    if let Some(handoff) = handoff.as_ref() {
        let task = &handoff.task;
        // __switch 返回这里前已恢复本 CPU 的 idle 上下文和内核根页表，
        // 因此换出的地址空间不再处于活动状态。
        task.clear_memory_set_current_hart_active();
        let was_running = task.is_running() && !task.termination_requested();
        if was_running {
            // 主动让出或被抢占的任务只有在上下文到达本 idle 循环后，
            // 才能对其他 CPU 可见。
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

/// 每个 CPU 永不返回的调度主循环，仅运行在该 CPU 的 idle 上下文和 idle 栈上。
///
/// 每轮首先提交上一任务的上下文交接并释放延迟销毁对象，再由全局调度器原子认领一个
/// 与当前 CPU 亲和性匹配的 Ready 任务。没有任务时，采用“发布 idle → 再检查队列 →
/// 开中断并 WFI”的顺序关闭入队与睡眠之间的丢失唤醒窗口。
///
/// 切入用户任务前必须发布地址空间在本硬件线程上的活动关系、设置每 CPU 内核状态并持有
/// 一个跨 `__switch` 的 Arc；任务换出后再结束 CPU 时间记账。任何新调度路径都不得绕过
/// idle 栈直接把仍在执行的上下文暴露给另一 CPU。
pub fn run_tasks() -> ! {
    loop {
        #[cfg(target_arch = "loongarch64")]
        crate::mm::ensure_kernel_space_active();
        // 只有换出任务的 __switch 已保存上下文且不再在本 CPU 执行后，才会到达这里。
        let handoff = publish_saved_handoff();
        // 换出上下文现已位于本 CPU 的 idle 栈，即使再无用户任务唤醒，
        // 也可以安全释放延迟销毁的 TCB。
        cleanup_dead_tasks();
        if crate::arch::smp::is_timer_service_hart() {
            crate::fs::ext4::flush_expired_lazytime_inodes_if_needed();
            crate::net::poll_background();
        }
        let mut next_task = fetch_task();
        // Linux 只在调度器选中不同于 prev 的任务时累计 nvcsw/nivcsw。
        // RespOS 为安全发布已保存上下文，总会经过每 CPU idle 栈；再次选中同一 Arc
        // 只是内部交接，不属于用户可观察的任务上下文切换。
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
                    // LoongArch 内核代码通常以 IE=0 运行。idle 不持锁，
                    // 因而可以开启中断，直到定时器或 IPI 将其唤醒。
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
            // 切换期间在 idle 栈上保留一个 Arc，使换出的确切任务即使已经退出并从
            // 任务管理器移除，仍能正确结束自己的 CPU 记账区间。
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
