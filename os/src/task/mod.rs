// os/src/task.rs

//! ### ~~进程~~任务模块
//!
//! 主要实现任务调度，实现 CPU 时间资源分配
//!
//! 至少现在，这里你可以将“进程”和“任务”作为同一个概念

mod context;
mod kstack;
mod manager;
mod tid;
mod processor;
mod task;
mod signal;
mod action;

use task::{TaskControlBlock, TaskStatus};
pub use context::TaskContext;
pub use manager::{add_task, pid2task, PID2TCB};
pub use processor::{
    current_task,
    current_trap_cx,
    current_user_token,
    run_tasks,
    schedule,
    take_current_task,
};
pub use signal::{SignalFlags, MAX_SIG};
pub use action::{SignalAction, SignalActions};
use crate::loader::get_app_data_by_name;
use lazy_static::lazy_static;
use alloc::sync::Arc;

lazy_static! {
    pub static ref INITPROC: Arc<TaskControlBlock> = {
        let initproc = Arc::new(
            TaskControlBlock::init(get_app_data_by_name("initproc").unwrap())
        );

        { &*PID2TCB }
            .lock()
            .insert(initproc.pid(), initproc.clone());

        initproc
    };
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

    task.op_memory_set_write(|memory_set| memory_set.recycle_data_pages());
    drop(task_inner);
    drop(task); // 该函数不会正常结束，手动删除引用
    // 到此为止应当仅有其父任务有其原子引用，父任务将其回收后，资源将会回收

    let mut _unused_task_cx = TaskContext::app_init_task_context(0, 0);
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


fn call_user_signal_handler(sig: usize, signal: SignalFlags) {
    let task = current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();

    let handler = task_inner.signal_actions.table[sig].handler;
    if handler != 0 {
        // user handler

        // handle flag
        task_inner.handling_sig = sig as isize;
        task_inner.signals ^= signal;

        // backup trapframe
        let trap_ctx = task.get_trap_cx();
        task_inner.trap_ctx_backup = Some(*trap_ctx);

        // modify trapframe
        trap_ctx.sepc = handler;

        // put args (a0)
        trap_ctx.x[10] = sig;
    } else {
        // default action
        println!("[K] task/call_user_signal_handler: default action: ignore it or kill process");
    }
}

fn call_kernel_signal_handler(signal: SignalFlags) {
    let task = current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    match signal {
        SignalFlags::SIGSTOP => {
            task_inner.frozen = true;
            task_inner.signals ^= SignalFlags::SIGSTOP;
        }
        SignalFlags::SIGCONT => {
            if task_inner.signals.contains(SignalFlags::SIGCONT) {
                task_inner.signals ^= SignalFlags::SIGCONT;
                task_inner.frozen = false;
            }
        }
        _ => {
            // println!(
            //     "[K] call_kernel_signal_handler:: current task sigflag {:?}",
            //     task_inner.signals
            // );
            task_inner.killed = true;
        }
    }
}

fn check_pending_signals() {
    for sig in 0..(MAX_SIG + 1) {
        let task = current_task().unwrap();
        let task_inner = task.inner_exclusive_access();
        let signal = SignalFlags::from_bits(1 << sig).unwrap();
        if task_inner.signals.contains(signal) && (!task_inner.signal_mask.contains(signal)) {
            let mut masked = true;
            let handling_sig = task_inner.handling_sig;
            if handling_sig == -1 {
                masked = false;
            } else {
                let handling_sig = handling_sig as usize;
                if !task_inner.signal_actions.table[handling_sig]
                    .mask
                    .contains(signal)
                {
                    masked = false;
                }
            }
            if !masked {
                drop(task_inner);
                drop(task);
                if signal == SignalFlags::SIGKILL
                    || signal == SignalFlags::SIGSTOP
                    || signal == SignalFlags::SIGCONT
                    || signal == SignalFlags::SIGDEF
                {
                    // signal is a kernel signal
                    call_kernel_signal_handler(signal);
                } else {
                    // signal is a user signal
                    call_user_signal_handler(sig, signal);
                    return;
                }
            }
        }
    }
}

pub fn handle_signals() {
    loop {
        check_pending_signals();
        let (frozen, killed) = {
            let task = current_task().unwrap();
            let task_inner = task.inner_exclusive_access();
            (task_inner.frozen, task_inner.killed)
        };
        if !frozen || killed {
            break;
        }
        suspend_current_and_run_next();
    }
}

pub fn check_signals_error_of_current() -> Option<(i32, &'static str)> {
    let task = current_task().unwrap();
    let task_inner = task.inner_exclusive_access();
    // println!(
    //     "[K] check_signals_error_of_current {:?}",
    //     task_inner.signals
    // );
    task_inner.signals.check_error()
}

pub fn current_add_signal(signal: SignalFlags) {
    let task = current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    task_inner.signals |= signal;
    // println!(
    //     "[K] current_add_signal:: current task sigflag {:?}",
    //     task_inner.signals
    // );
}