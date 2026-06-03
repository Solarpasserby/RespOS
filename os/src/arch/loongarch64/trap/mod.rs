// os/src/arch/loongarch64/trap/mod.rs

mod context;

use super::register::{badv, ecfg, eentry, estat};
use super::{sbi::clear_timer_interrupt, timer::set_next_ti_trigger};
use crate::syscall::*;
use crate::task::{current_task, exit_and_run_next, handle_signals, yield_current_task};
use core::arch::global_asm;

pub use context::TrapContext;

const PTHREAD_FAULT_TRACE: bool = false;
const PTHREAD_TLS_FAULT_ERA: usize = 0x120004c50;
const PTHREAD_NULL_TCB_FAULT_ERA: usize = 0x12001be94;

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

fn handle_user_page_fault(cx: &TrapContext, exception: estat::Exception) {
    let badv = badv::read();
    if PTHREAD_FAULT_TRACE
        && (cx.era == PTHREAD_TLS_FAULT_ERA || cx.era == PTHREAD_NULL_TCB_FAULT_ERA)
    {
        let (tid, tgid) = current_task()
            .map(|task| (task.tid(), task.tgid()))
            .unwrap_or((usize::MAX, usize::MAX));
        println!(
            "[la-pthread-trace] fault tid={} tgid={} cause={:?} era={:#x} badv={:#x} tp={:#x} sp={:#x} a0={:#x} a1={:#x} a2={:#x} a3={:#x}",
            tid,
            tgid,
            estat::Trap::Exception(exception),
            cx.era,
            badv,
            cx.x[2],
            cx.x[3],
            cx.x[4],
            cx.x[5],
            cx.x[6],
            cx.x[7]
        );
    }
    let result = current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_write(|memory_set| {
            memory_set.handle_page_fault(page_fault_cause(exception), badv)
        });
    if let Err(err) = result {
        println!(
            "[kernel] PageFault in application, cause = {:?}, era = {:#x}, bad addr = {:#x}, err = {:?}, kernel killed it.",
            estat::Trap::Exception(exception),
            cx.era,
            badv,
            err
        );
        exit_and_run_next(-2);
    }
}

fn handle_user_syscall(cx: &mut TrapContext) {
    cx.era += 4;
    cx.x[4] = match syscall(cx.syscall_id(), cx.syscall_args()) {
        Ok(ret) => ret,
        Err(err) => err.as_ret() as usize,
    };
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
    match estat::cause(estat::read()) {
        estat::Trap::Interrupt(estat::Interrupt::Timer) => {
            clear_timer_interrupt();
            set_next_ti_trigger();
            yield_current_task();
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            handle_user_syscall(cx);
        }
        estat::Trap::Exception(exception) if is_page_fault(exception) => {
            handle_user_page_fault(cx, exception);
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
    handle_signals();
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
            println!("[kernel] Timer interrupt in kernel mode");
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
