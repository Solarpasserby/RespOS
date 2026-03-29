// os/src/task.rs

//! ### ~~进程~~任务模块
//! 
//! 主要实现任务调度，实现 CPU 时间资源分配
//! 
//! 至少现在，这里你可以将“进程”和“任务”作为同一个概念

mod context;
mod task;
mod manager;
mod switch;
mod pid;
mod processor;

use lazy_static::lazy_static;
use alloc::sync::Arc;
use crate::loader::get_app_data_by_name;
use context::TaskContext;
use task::{ TaskControlBlock, TaskStatus };
pub use manager::add_task;
pub use processor::{
    current_task,
    current_user_token,
    current_trap_cx,
    take_current_task,
    run_tasks ,schedule
};

lazy_static! {
    pub static ref INITPROC: Arc<TaskControlBlock> = Arc::new(
        TaskControlBlock::new(get_app_data_by_name("initproc").unwrap())
    );
}

pub fn add_initproc() {
    add_task(INITPROC.clone());
}

/// 退出当前任务并运行下一个任务
pub fn exit_current_and_run_next(exit_code: i32) {
    // 由于函数 run_tasks 是死循环，因此实际上只需要终止当前任务
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    task_inner.task_status = TaskStatus::Exited; // 修改任务状态
    task_inner.exit_code = exit_code; // 设置任务退出码

    // 父任务结束后，将其子任务添加为初始任务的子任务
    {
        let mut initproc_inner = INITPROC.inner_exclusive_access();
        for child in task_inner.children.iter() {
            child.inner_exclusive_access().parent = Some(Arc::downgrade(&INITPROC));
            initproc_inner.children.push(child.clone());
        }
    }
    task_inner.children.clear();

    task_inner.memory_set.recycle_data_pages();
    drop(task_inner);
    drop(task); // 该函数不会正常结束，手动删除引用

    let mut _unused_task_cx = TaskContext::init_zero();
    // 切换到空闲任务，实际上永远不会切换回来，这片内核栈也会被回收
    schedule(&mut _unused_task_cx as *mut _);
}

/// 停止当前任务并运行下一个任务
pub fn suspend_current_and_run_next() {
    // 由于函数 run_tasks 是死循环，因此实际上只需要暂停当前任务
    // 根据设计，任务调度拆分为两部分管理：一个是 manager，一个是 processor
    // 实现两部分的方法即可，同时注意修改任务的状态
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_context as *mut TaskContext;
    task_inner.task_status = TaskStatus::Ready; // 修改任务状态
    drop(task_inner);

    add_task(task); // 在 TASK_MANAGER 队列中添加任务
    schedule(task_cx_ptr); // 切换到空闲任务
}
