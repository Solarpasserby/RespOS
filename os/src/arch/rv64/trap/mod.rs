// os/src/trap.rs

//! ### ~~中断~~异常模块
//!
//! 注：应当注意到目前内核台下触发中断会被屏蔽
//! 因此无需担心某些过程是否需要关闭中断

mod context;

use super::timer::set_next_ti_trigger;
use crate::signal::{SiField, Sig, SigInfo};
use crate::syscall::*;
use crate::task::{current_task, exit_and_run_next, handle_signals, yield_current_task};
use core::arch::global_asm;
use riscv::register::{
    mtvec::TrapMode,
    scause::{self, Exception, Interrupt, Trap},
    sie,
    sstatus::{self, SPP},
    stval, stvec,
};

pub use context::TrapContext;

global_asm!(include_str!("trap.S"));

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PageFaultCause {
    Instruction,
    Load,
    Store,
}

fn page_fault_cause(cause: Trap) -> Option<PageFaultCause> {
    match cause {
        Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::InstructionPageFault) => Some(PageFaultCause::Instruction),
        Trap::Exception(Exception::LoadFault) | Trap::Exception(Exception::LoadPageFault) => {
            Some(PageFaultCause::Load)
        }
        Trap::Exception(Exception::StoreFault) | Trap::Exception(Exception::StorePageFault) => {
            Some(PageFaultCause::Store)
        }
        _ => None,
    }
}

unsafe extern "C" {
    fn __trap_from_user();
    fn __trap_from_kernel();
    pub fn __restore() -> !;
}

pub fn init() {
    let mut sstatus = sstatus::read();
    sstatus.set_spp(SPP::Supervisor);
    unsafe {
        stvec::write(__trap_from_kernel as usize, TrapMode::Direct);
    }
}

pub fn enable_timer_interrupt() {
    unsafe {
        sie::set_stimer();
    }
}

/// ~~中断~~异常处理函数
///
/// 用户程序上下文保存于内核栈上，包含用户程序使用的寄存器数据以及系统调用传递的寄存器参数
///
/// 该函数根据 `CSR` 区分不同异常类型，对不同类型异常做不同处理
#[unsafe(no_mangle)]
pub fn trap_handler(cx: &mut TrapContext) {
    // 设置状态寄存器，使内核可以访问用户数据
    unsafe {
        sstatus::set_sum();
    }
    let scause = scause::read();
    let stval = stval::read();
    match scause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            cx.sepc += 4; // 异常处理完成后直接执行后续指令
            cx.x[10] = match syscall(
                cx.x[17],
                [cx.x[10], cx.x[11], cx.x[12], cx.x[13], cx.x[14], cx.x[15]],
            ) {
                Ok(ret) => ret,
                Err(err) => err.as_ret() as usize,
            };
        }
        Trap::Exception(Exception::StoreFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::InstructionPageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::LoadPageFault) => {
            let page_fault_cause = page_fault_cause(scause.cause()).unwrap();
            let result = current_task()
                .expect("[kernel] current task is None.")
                .op_memory_set_write(|memory_set| {
                    memory_set.handle_page_fault(page_fault_cause, stval)
                });
            if result.is_err() {
                let siginfo = SigInfo::new(Sig::SIGSEGV.raw(), SigInfo::KERNEL, SiField::None);
                current_task()
                    .expect("[kernel] current task is None.")
                    .receive_siginfo(siginfo, true);
            }
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            println!(
                "[kernel] IllegalInstruction in application, cause = {:?}, sepc = {:#x}, bad addr = {:#x}, kernel killed it.",
                scause.cause(),
                cx.sepc,
                stval
            );
            // 非法指令退出码
            exit_and_run_next(-3);
        }
        Trap::Exception(Exception::Breakpoint) => {
            println!(
                "[kernel] Breakpoint in application at sepc={:#x}, kernel killed it.",
                cx.sepc
            );
            exit_and_run_next(-4);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_ti_trigger();
            yield_current_task();
        }
        _ => {
            panic!(
                "Unsupported trap {:?}, stval = {:#?}!",
                scause.cause(),
                stval
            );
        }
    };
    handle_signals();
    return;
}

#[unsafe(no_mangle)]
pub fn kernel_trap_handler(cx: &mut TrapContext) {
    let scause = scause::read();
    match scause.cause() {
        Trap::Exception(Exception::Breakpoint) => {
            info!("Breakpoint at 0x{:x}", cx.sepc);
            cx.sepc += 2;
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            panic!("IllegalInstruction at 0x{:x}", cx.sepc);
        }
        Trap::Exception(Exception::LoadPageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::StoreFault) => {
            panic!(
                "page fault in kernel, sepc = {:#x}, bad addr = {:#x}, scause = {:?}",
                cx.sepc,
                stval::read(),
                scause.cause()
            );
        }
        Trap::Exception(Exception::InstructionPageFault) => {
            panic!(
                "Instruction page fault at 0x{:x}, badaddr = {:#x}",
                cx.sepc,
                stval::read()
            );
        }
        Trap::Exception(Exception::UserEnvCall) => {
            panic!("UserEnvCall from kernel!");
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            info!("SupervisorTimer in kernel mode");
        }
        _ => {
            panic!(
                "Unsupported trap {:?}, stval = {:#x}!",
                scause.cause(),
                stval::read()
            );
        }
    }
}
