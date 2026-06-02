// os/src/arch/loongarch64/trap/mod.rs

mod context;

use super::register::{badv, ecfg, eentry, era, estat};
use super::{sbi::clear_timer_interrupt, timer::set_next_ti_trigger};
use crate::syscall::*;
use crate::task::{current_task, exit_and_run_next, handle_signals, yield_current_task};
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

global_asm!(include_str!("trap.S"));

unsafe extern "C" {
    fn __trap_from_user();
    fn __trap_from_kernel();
    pub fn __restore() -> !;
}

pub fn init() {
    // 设置异常入口点为 __trap_from_kernel（内核初始化期间）
    unsafe {
        eentry::write(__trap_from_kernel as usize);
    }
}

pub fn enable_timer_interrupt() {
    unsafe {
        ecfg::enable_timer_interrupt();
        // Keep kernel-mode interrupts disabled. User contexts re-enable IE through PRMD.PIE
        // when __restore executes ERTN.
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
        estat::Trap::Interrupt(interrupt) => {
            panic!(
                "[kernel] Unsupported interrupt: {:?}, era = {:#x}",
                interrupt, cx.era
            );
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            cx.era += 4;
            let id = cx.syscall_id();
            let args = cx.syscall_args();
            cx.x[4] = match syscall(id, args) {
                Ok(ret) => ret,
                Err(err) => err.as_ret() as usize,
            };
        }
        estat::Trap::Exception(exception @ (
            estat::Exception::PageInvalidFetch
            | estat::Exception::PageInvalidLoad
            | estat::Exception::PageInvalidStore
            | estat::Exception::PageModifyFault
            | estat::Exception::PageNonReadable
            | estat::Exception::PageNonExecutable
            | estat::Exception::PagePrivilegeIllegal
        )) => {
            let badv = badv::read();
            let result = current_task()
                .expect("[kernel] current task is None.")
                .op_memory_set_write(|memory_set| {
                    memory_set.handle_page_fault(page_fault_cause(exception), badv)
                });
            if let Err(err) = result {
                println!(
                    "[kernel] PageFault in application, cause = {:?}, era = {:#x}, bad addr = {:#x}, err = {:?}, kernel killed it.",
                    estat::cause(estat::read()),
                    cx.era,
                    badv,
                    err
                );
                exit_and_run_next(-2);
            }
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
pub fn trap_from_kernel() -> ! {
    let estat = estat::read();
    let era = era::read();
    let badv = badv::read();
    panic!(
        "[kernel] Trap is not defined in kernel: cause = {:?}, estat = {:#x}, era = {:#x}, badv = {:#x}",
        estat::cause(estat),
        estat,
        era,
        badv
    );
}
