// os/src/task/processor.rs

//! #### 任务调度之 CPU 状态转换
//! 
//! - 功能：processor 可以依据调度策略切换任务上下文转而执行目标任务
//! 
//! - 理解：
//!     - 上下文切换的关键是 [`__switch`] 函数，该函数保存和恢复

use lazy_static::lazy_static;
use alloc::sync::Arc;
use crate::sync::UPSafeCell;
use crate::trap::TrapContext;
use super::task::{ TaskControlBlock, TaskStatus };
use super::manager::fetch_task;
use super::switch::__switch;
use super::context::TaskContext;

lazy_static! {
    pub static ref PROCESSOR: UPSafeCell<Processor> = unsafe {
        UPSafeCell::new(Processor::new())
    };
}

/// 处理器管理
/// 
/// 管理维护 CPU 状态
pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    idle_task_cx: TaskContext, // 空闲任务上下文，无任务调度时切换到该任务上下文
}

impl Processor {
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: TaskContext::init_zero(),
        }
    }

    /// 取出当前执行的任务
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }

    /// 返回当前执行的任务的一份拷贝
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(|task| Arc::clone(task))
    }

    /// 获取空闲任务上下文的可变引用
    pub fn get_idle_task_cx(&mut self) -> *mut TaskContext {
        &mut self.idle_task_cx as *mut TaskContext
    }
}

/// 取出当前执行的任务
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().take_current()
}

/// 获取当前执行的任务的一份拷贝
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().current()
}

/// 获取当前执行的任务的 `stap` 寄存器值
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    let token = task.inner_exclusive_access().get_user_token();
    token
}

/// 获取当前执行的任务的异常上下文
/// 
/// 生命周期警告，可变引用
pub fn current_trap_cx() -> &'static mut TrapContext {
    current_task().unwrap().inner_exclusive_access().get_trap_cx()
}

/// 运行任务
/// 
/// 死循环，不断尝试将任务队列中的任务载入执行
/// 
/// 该函数仅被空闲任务调用，因此任务调度的内容只会出现在初始栈上
pub fn run_tasks() {
    loop {
        let mut processor = PROCESSOR.exclusive_access();
        if let Some(task) = fetch_task() {
            let idle_task_cx_ptr = processor.get_idle_task_cx();
            let mut task_inner = task.inner_exclusive_access();
            let next_task_cx_ptr = &task_inner.task_context as *const TaskContext;
            task_inner.task_status = TaskStatus::Running;
            drop(task_inner); // 销毁借用
            processor.current = Some(task);
            drop(processor); // 销毁借用
            unsafe {
                __switch(
                    idle_task_cx_ptr,
                    next_task_cx_ptr,
                );
            }
        }
    }
}

/// 任务安排——切换任务到空闲任务
/// 
/// 这一设计让其他用户任务不再涉及任务调度相关内容
/// 
/// > 使得调度机制对于换出进程的 Trap 执行流是不可见的，
/// > 它在决定换出的时候只需调用 schedule 而无需操心调度的事情，
/// > 从而各执行流的分工更加明确了，虽然带来了更大的开销
pub fn schedule(switched_task_cx_ptr: *mut TaskContext) {
    let mut processor = PROCESSOR.exclusive_access();
    let idle_task_cx_ptr = processor.get_idle_task_cx();
    drop(processor);
    unsafe {
        __switch(
            switched_task_cx_ptr,
            idle_task_cx_ptr,
        );
    }
}
