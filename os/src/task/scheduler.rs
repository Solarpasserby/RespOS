//! #### 任务调度队列
//!
//! 调度器维护 FIFO 就绪队列，并在主动让出/阻塞/退出时选择下一个任务。
//! 当前架构层 `__switch` 接收下一个任务的内核栈指针，因此这里会完成最后一步切换。

use super::processor::{PROCESSOR, current_task};
use super::task::{TaskControlBlock, task_exit};
use crate::{arch::task::__switch, mutex::SpinNoIrqLock};
use alloc::{collections::vec_deque::VecDeque, sync::Arc};
use bitflags::bitflags;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref SCHEDULER: SpinNoIrqLock<Scheduler> = SpinNoIrqLock::new(Scheduler::new());
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

/// 阻塞任务。
pub fn block_task(task: Arc<TaskControlBlock>) {
    assert!(task.is_blocked());
    SCHEDULER.lock().block(task);
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
pub fn switch_to_next_task() -> ! {
    let Some(current) = current_task() else {
        crate::arch::idle();
    };

    if let Some(next_task) = fetch_task() {
        let next_task_kernel_stack = next_task.kstack();
        let current_task_ptr = Arc::as_ptr(&current) as usize;
        next_task.set_running();
        PROCESSOR.lock().switch_to(next_task);
        unsafe {
            __switch(next_task_kernel_stack, current_task_ptr);
        }
    }

    crate::arch::idle();
}

/// 主动让出当前任务。
#[unsafe(no_mangle)]
pub fn yield_current_task() {
    let Some(task) = current_task() else {
        return;
    };

    if let Some(next_task) = fetch_task() {
        let current_task_ptr = Arc::as_ptr(&task) as usize;
        task.set_ready();
        add_task(task);

        let next_task_kernel_stack = next_task.kstack();
        next_task.set_running();
        PROCESSOR.lock().switch_to(next_task);
        unsafe {
            __switch(next_task_kernel_stack, current_task_ptr);
        }
    }
}

/// 阻塞当前任务并运行下一个任务。
#[unsafe(no_mangle)]
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
        unsafe {
            __switch(next_task_kernel_stack, current_task_ptr);
        }
    }
}

#[unsafe(no_mangle)]
pub fn exit_and_run_next(exit_code: i32) -> ! {
    let Some(task) = current_task() else {
        crate::arch::idle();
    };

    // 当前任务本就不在调度队列中，无需删除
    task_exit(task, exit_code);
    switch_to_next_task();
}

/// FIFO 任务调度器。
pub struct Scheduler {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
    blocked_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Scheduler {
    /// 创建一个空调度器。
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            blocked_queue: VecDeque::new(),
        }
    }

    /// 添加任务到调度器就绪队列。
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }

    /// 取出调度器就绪队列队首任务。
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.ready_queue.pop_front()
    }

    /// 从调度器就绪队列中移除任务。
    pub fn remove(&mut self, tid: usize) {
        self.ready_queue.retain(|task| task.tid() != tid);
    }

    /// 从调度器就绪队列中移除线程组。
    pub fn remove_thread_group(&mut self, tgid: usize) {
        self.ready_queue.retain(|task| task.tgid() != tgid);
    }

    /// 阻塞任务。
    pub fn block(&mut self, task: Arc<TaskControlBlock>) {
        self.blocked_queue.push_back(task);
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
