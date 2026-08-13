// os/src/arch/loongarch64/trap/mod.rs

mod context;

use super::register::{badv, ecfg, eentry, estat};
use super::{sbi::clear_timer_interrupt, timer::set_next_ti_trigger};
use crate::signal::{SiField, Sig, SigInfo};
use crate::syscall::*;
use crate::task::{
    current_task, exit_and_run_next, exit_by_signal_and_run_next, handle_signals,
    preempt_current_task,
};
use core::arch::global_asm;

pub use context::TrapContext;

/// 页错误原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultCause {
    Instruction,
    Load,
    Store,
}

fn page_fault_cause(exception: estat::Exception) -> PageFaultCause {
    match exception {
        estat::Exception::PageInvalidFetch | estat::Exception::PageNonExecutable => {
            PageFaultCause::Instruction
        }
        estat::Exception::PageInvalidStore | estat::Exception::PageModifyFault => {
            PageFaultCause::Store
        }
        _ => PageFaultCause::Load,
    }
}

fn is_page_fault(exception: estat::Exception) -> bool {
    matches!(
        exception,
        estat::Exception::PageInvalidFetch
            | estat::Exception::PageInvalidLoad
            | estat::Exception::PageInvalidStore
            | estat::Exception::PageModifyFault
            | estat::Exception::PageNonReadable
            | estat::Exception::PageNonExecutable
            | estat::Exception::PagePrivilegeIllegal
    )
}

fn handle_user_page_fault(_cx: &TrapContext, exception: estat::Exception) {
    let badv = badv::read();
    let cause = page_fault_cause(exception);
    let result = current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_write(|memory_set| memory_set.handle_page_fault(cause, badv));
    if let Err(err) = result {
        let task = current_task().expect("[kernel] current task is None.");
        #[cfg(feature = "fault_trace")]
        println!(
            "[user-fault] tid={} tgid={} cause={:?} era={:#x} badi={:#010x} badv={:#x} a0={:#x} a1={:#x} a2={:#x} a3={:#x} sp={:#x} ra={:#x} err={:?}",
            task.tid(),
            task.tgid(),
            cause,
            _cx.era,
            read_badi(),
            badv,
            _cx.x[4],
            _cx.x[5],
            _cx.x[6],
            _cx.x[7],
            _cx.x[3],
            _cx.x[1],
            err
        );
        let sig = if err == Errno::EIO {
            Sig::SIGBUS
        } else {
            Sig::SIGSEGV
        };
        if task.op_sig_pending(|pending| pending.mask.contain_signal(sig)) {
            exit_by_signal_and_run_next(sig.raw());
        }
        let siginfo = SigInfo::new(sig.raw(), SigInfo::KERNEL, SiField::None);
        task.receive_siginfo(siginfo, true);
    }
}

fn handle_user_syscall(cx: &mut TrapContext) -> Option<usize> {
    let syscall_id = cx.syscall_id();
    let syscall_args = cx.syscall_args();
    cx.era += 4;
    let ret = syscall(syscall_id, syscall_args);
    if syscall_id == SYSCALL_SIGRETURN && ret.is_ok() {
        return None;
    }

    let restart_syscall_arg0 =
        (ret == Err(Errno::EINTR) && syscall_id == SYSCALL_WAIT4).then_some(syscall_args[0]);

    cx.x[4] = match ret {
        Ok(ret) => ret,
        Err(err) => err.as_ret() as usize,
    };
    restart_syscall_arg0
}

global_asm!(include_str!("trap.S"));

unsafe extern "C" {
    fn __trap_from_user();
    fn __trap_from_kernel();
    pub fn __restore() -> !;
}

pub fn init() {
    // 初始化阶段尚未准备用户上下文，异常先进入内核 trap 路径。
    unsafe {
        eentry::write(__trap_from_kernel as usize);
    }
}

pub fn enable_timer_interrupt() {
    unsafe {
        ecfg::enable_timer_interrupt();
        // 内核态保持关中断；用户态通过 PRMD.PIE 在 ERTN 后重新开中断。
        super::register::crmd::set_interrupt_enabled(false);
    }
}

#[inline]
fn read_badi() -> usize {
    let bits: usize;
    unsafe {
        core::arch::asm!("csrrd {}, 0x8", out(reg) bits, options(nomem, nostack));
    }
    bits
}

/// 异常处理入口
#[unsafe(no_mangle)]
pub fn trap_handler(cx: &mut TrapContext) {
    crate::perf::user_trap(1);
    // 未使用过 FP/LSX 的任务在汇编入口跳过扩展状态保存。任务首次
    // 触发 FPD/SXD 后仍采用 eager save/restore，以保持跨核迁移语义简单可靠。
    if cx.extension_state_active() {
        crate::perf::extension_state_eager_save(1);
    }
    let mut restart_syscall_arg0 = None;
    match estat::cause(estat::read()) {
        estat::Trap::Interrupt(estat::Interrupt::Timer) => {
            crate::perf::user_timer_trap(1);
            crate::perf::sample_concurrency();
            clear_timer_interrupt();
            set_next_ti_trigger();
            if crate::arch::smp::is_timer_service_hart() {
                crate::timer::await_task_timer_deadline();
                check_all_task_timers();
            }
            preempt_current_task();
        }
        estat::Trap::Interrupt(estat::Interrupt::Ipi) => {
            crate::perf::user_ipi_trap(1);
            crate::arch::smp::acknowledge_ipi();
            crate::timer::rearm_task_timer_request();
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            crate::perf::user_syscall_trap(1);
            restart_syscall_arg0 = handle_user_syscall(cx);
        }
        estat::Trap::Exception(exception) if is_page_fault(exception) => {
            crate::perf::user_page_fault_trap(1);
            handle_user_page_fault(cx, exception);
        }
        estat::Trap::Exception(
            estat::Exception::FloatingPointUnavailable | estat::Exception::SimdUnavailable,
        ) => {
            // __restore enables FP/LSX and retries the faulting instruction.
            cx.activate_extension_state();
        }
        estat::Trap::Exception(estat::Exception::IllegalInstruction) => {
            let inst = read_badi();
            let tid = current_task().map(|task| task.tid()).unwrap_or(usize::MAX);
            println!(
                "[kernel] IllegalInstruction in application, tid = {}, era = {:#x}, badi = {:#x}, kernel killed it.",
                tid, cx.era, inst
            );
            exit_and_run_next(-3);
        }
        estat::Trap::Exception(estat::Exception::Breakpoint) => {
            println!(
                "[kernel] Breakpoint in application at era={:#x}, kernel killed it.",
                cx.era
            );
            exit_and_run_next(-4);
        }
        estat::Trap::Interrupt(interrupt) => {
            panic!(
                "[kernel] Unsupported interrupt: {:?}, era = {:#x}",
                interrupt, cx.era
            );
        }
        cause => {
            let badv = badv::read();
            panic!(
                "Unsupported trap: cause = {:?}, era = {:#x}, badv = {:#x}!",
                cause, cx.era, badv
            );
        }
    }
    handle_signals(restart_syscall_arg0);
}

#[unsafe(no_mangle)]
pub fn trap_from_kernel(cx: &mut TrapContext) {
    match estat::cause(estat::read()) {
        estat::Trap::Exception(estat::Exception::Breakpoint) => {
            println!("[kernel] Breakpoint at 0x{:x}", cx.era);
            cx.era += 4; // LoongArch break 指令为 4 字节
        }
        estat::Trap::Exception(estat::Exception::IllegalInstruction) => {
            panic!("[kernel] IllegalInstruction at 0x{:x}", cx.era);
        }
        estat::Trap::Exception(exception) if is_page_fault(exception) => {
            panic!(
                "[kernel] page fault in kernel, era = {:#x}, badaddr = {:#x}, cause = {:?}",
                cx.era,
                badv::read(),
                estat::Trap::Exception(exception)
            );
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            panic!("[kernel] Syscall from kernel!");
        }
        estat::Trap::Interrupt(estat::Interrupt::Timer) => {
            crate::perf::sample_concurrency();
            clear_timer_interrupt();
            set_next_ti_trigger();
            if crate::arch::smp::is_timer_service_hart() && crate::task::current_task().is_none() {
                crate::timer::await_task_timer_deadline();
                check_all_task_timers();
            }
        }
        estat::Trap::Interrupt(estat::Interrupt::Ipi) => {
            crate::arch::smp::acknowledge_ipi();
            crate::timer::rearm_task_timer_request();
        }
        cause => {
            panic!(
                "[kernel] Unsupported trap in kernel: cause = {:?}, era = {:#x}, badv = {:#x}!",
                cause,
                cx.era,
                badv::read()
            );
        }
    }
}
