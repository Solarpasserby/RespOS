// os/src/task/processor.rs

//! #### 任务调度之 CPU 状态转换
//!
//! - 功能：processor 可以依据调度策略切换任务上下文转而执行目标任务
//!
//! - 理解：上下文切换的关键是 [`__switch`] 函数，该函数保存和恢复

use super::scheduler::fetch_task;
use super::task::TaskControlBlock;
use crate::arch::task::__switch;
use crate::mutex::SpinNoIrqLock;
use alloc::sync::Arc;
use lazy_static::lazy_static;

// 空闲任务
lazy_static! {
    pub static ref IDLE_TASK: Arc<TaskControlBlock> = Arc::new(TaskControlBlock::zero_init());
}

lazy_static! {
    pub static ref PROCESSOR: SpinNoIrqLock<Processor> = SpinNoIrqLock::new(Processor::new());
}

/// 处理器管理
///
/// 管理维护 CPU 状态
pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
}

impl Processor {
    pub fn new() -> Self {
        Self { current: None }
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
        self.current = Some(task);
    }
}

/// 取出当前执行的任务
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.lock().take_current()
}

/// 获取当前执行的任务的一份拷贝
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.lock().current()
}

/// 获取当前执行的任务的页表基址寄存器值
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    let token = task.get_user_token();
    token
}

/// 运行任务
///
/// 该函数仅被空闲任务调用，因此任务调度的内容只会出现在初始栈上
pub fn run_tasks() {
    loop {
        if let Some(next_task) = fetch_task() {
            let idle_task = IDLE_TASK.clone();
            let next_task_kstack = next_task.kstack();
            let idle_task_ptr = Arc::as_ptr(&idle_task) as usize;
            idle_task.set_ready();
            next_task.set_running();
            let mut processor = PROCESSOR.lock();
            processor.current = Some(next_task.clone());
            drop(processor);
            // 事实上这个循环只会执行一次，这里需要释放 `next_task` 的引用
            drop(next_task);
            unsafe {
                __switch(next_task_kstack, idle_task_ptr);
            }
            unreachable!("Unreachable in run_tasks");
        }
    }
}
