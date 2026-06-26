// os/src/task.rs

//! ### ~~进程~~任务模块
//!
//! 主要实现任务调度，实现 CPU 时间资源分配
//!
//! 至少现在，这里你可以将“进程”和“任务”作为同一个概念

mod aux;
mod context;
mod futex;
mod kstack;
mod manager;
mod processor;
mod scheduler;
mod task;
mod tid;

use crate::loader::get_app_data_by_name;
use alloc::sync::Arc;
pub use aux::*;
pub use context::TaskContext;
pub use futex::{check_futex_timeouts, do_futex};
use lazy_static::lazy_static;
pub use manager::TASK_MANAGER;
pub use processor::{current_task, current_user_token, run_tasks, take_current_task};
pub use scheduler::{
    WaitOption, add_task, block_task, blocking_and_run_next, exit_and_run_next,
    exit_by_signal_and_run_next, exit_group_and_run_next, fetch_task, preempt_current_task,
    prepare_current_task_blocked, remove_task, remove_thread_group, requeue_ready_task,
    stop_current_and_run_next, switch_to_next_task, wakeup_stopped_task, wakeup_task,
    yield_current_task,
};
pub use task::TaskStatus;
pub use task::{CloneFlags, TaskControlBlock};

lazy_static! {
    pub static ref INITPROC: Arc<TaskControlBlock> = {
        let data = get_app_data_by_name("initproc").unwrap();
        TaskControlBlock::init(data)
    };
}

pub fn add_initproc() {
    add_task(INITPROC.clone());
}

pub fn handle_signals() {
    crate::signal::handle_signal();
}
