// os/src/task.rs

//! ### ~~进程~~任务模块
//!
//! 主要实现任务调度，实现 CPU 时间资源分配
//!
//! 至少现在，这里你可以将“进程”和“任务”作为同一个概念

mod action;
mod context;
mod kstack;
mod manager;
mod processor;
mod scheduler;
mod signal;
mod task;
mod tid;

#[cfg(target_arch = "riscv64")]
use crate::loader::get_app_data_by_name;
pub use action::{SignalAction, SignalActions};
use alloc::sync::Arc;
pub use context::TaskContext;
use lazy_static::lazy_static;
pub use manager::TASK_MANAGER;
pub use processor::{current_task, current_user_token, run_tasks, take_current_task};
pub use scheduler::{
    WaitOption, add_task, block_task, blocking_and_run_next, exit_and_run_next, fetch_task,
    remove_task, remove_thread_group, switch_to_next_task, yield_current_task,
};
pub use signal::{MAX_SIG, SignalFlags};
pub use task::{CloneFlags, TaskControlBlock};

lazy_static! {
    pub static ref INITPROC: Arc<TaskControlBlock> = {
        #[cfg(target_arch = "riscv64")]
        let data = get_app_data_by_name("initproc").unwrap();
        #[cfg(target_arch = "loongarch64")]
        let data = &[][..];

        TaskControlBlock::init(data)
    };
}

pub fn add_initproc() {
    add_task(INITPROC.clone());
}

fn call_user_signal_handler(sig: usize, signal: SignalFlags) {
    let task = current_task().unwrap();
    let mut signal_inner = task.get_signal_inner();

    let handler = signal_inner.signal_actions.table[sig].handler;
    if handler != 0 {
        // user handler

        // handle flag
        signal_inner.handling_sig = sig as isize;
        signal_inner.signals ^= signal;

        // backup trapframe
        let trap_ctx = task.get_trap_cx();
        signal_inner.trap_ctx_backup = Some(*trap_ctx);

        // modify trapframe
        trap_ctx.set_sepc(handler);

        // put args (a0)
        trap_ctx.set_a0(sig);
    } else {
        // default action
        println!("[K] task/call_user_signal_handler: default action: ignore it or kill process");
    }
}

fn call_kernel_signal_handler(signal: SignalFlags) {
    let task = current_task().unwrap();
    let mut signal_inner = task.get_signal_inner();
    match signal {
        SignalFlags::SIGSTOP => {
            signal_inner.frozen = true;
            signal_inner.signals ^= SignalFlags::SIGSTOP;
        }
        SignalFlags::SIGCONT => {
            if signal_inner.signals.contains(SignalFlags::SIGCONT) {
                signal_inner.signals ^= SignalFlags::SIGCONT;
                signal_inner.frozen = false;
            }
        }
        _ => {
            // println!(
            //     "[K] call_kernel_signal_handler:: current task sigflag {:?}",
            //     signal_inner.signals
            // );
            signal_inner.killed = true;
        }
    }
}

fn check_pending_signals() {
    for sig in 0..(MAX_SIG + 1) {
        let task = current_task().unwrap();
        let signal_inner = task.get_signal_inner();
        let signal = SignalFlags::from_bits(1 << sig).unwrap();
        if signal_inner.signals.contains(signal) && (!signal_inner.signal_mask.contains(signal)) {
            let mut masked = true;
            let handling_sig = signal_inner.handling_sig;
            if handling_sig == -1 {
                masked = false;
            } else {
                let handling_sig = handling_sig as usize;
                if !signal_inner.signal_actions.table[handling_sig]
                    .mask
                    .contains(signal)
                {
                    masked = false;
                }
            }
            if !masked {
                drop(signal_inner);
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
            let signal_inner = task.get_signal_inner();
            (signal_inner.frozen, signal_inner.killed)
        };
        if !frozen || killed {
            break;
        }
        yield_current_task();
    }
}

pub fn check_signals_error_of_current() -> Option<(i32, &'static str)> {
    let task = current_task().unwrap();
    let signal_inner = task.get_signal_inner();
    // println!(
    //     "[K] check_signals_error_of_current {:?}",
    //     signal_inner.signals
    // );
    signal_inner.signals.check_error()
}

pub fn current_add_signal(signal: SignalFlags) {
    let task = current_task().unwrap();
    let mut signal_inner = task.get_signal_inner();
    signal_inner.signals |= signal;
    // println!(
    //     "[K] current_add_signal:: current task sigflag {:?}",
    //     signal_inner.signals
    // );
}
