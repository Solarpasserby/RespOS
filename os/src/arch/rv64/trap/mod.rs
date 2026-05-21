// os/src/trap.rs

//! ### ~~中断~~异常模块
//! 
//! 注：应当注意到目前内核台下触发中断会被屏蔽
//! 因此无需担心某些过程是否需要关闭中断

mod context;

use riscv::register::{
    mtvec::TrapMode,
    stvec, stval, sie,
    sstatus::{self, SPP},
    scause::{self, Trap, Exception, Interrupt},
};
use core::arch::global_asm;
use crate::syscall::*;
use crate::task::{suspend_current_and_run_next, exit_current_and_run_next, handle_signals};
use crate::timer::set_next_ti_trigger;

pub use context::TrapContext;


global_asm!(include_str!("trap.S"));

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
    unsafe { sie::set_stimer(); }
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
        Trap::Exception(Exception::StoreFault) |
        Trap::Exception(Exception::StorePageFault) |
        Trap::Exception(Exception::InstructionFault) |
        Trap::Exception(Exception::InstructionPageFault) |
        Trap::Exception(Exception::LoadFault) |
        Trap::Exception(Exception::LoadPageFault) => {
            println!(
                "[kernel] PageFault in application, cause = {:?}, sepc = {:#x}, bad addr = {:#x}, kernel killed it.",
                scause.cause(),
                cx.sepc,
                stval
            );
            // 页错误退出码
            exit_current_and_run_next(-2);
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            println!("[kernel] IllegalInstruction in application, kernel killed it.");
            // 非法指令退出码
            exit_current_and_run_next(-3);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_ti_trigger();
            suspend_current_and_run_next();
        }
        _ => {
            panic!("Unsupported trap {:?}, stval = {:#?}!", scause.cause(), stval);
        }
    };
    handle_signals();
    return;
}


#[unsafe(no_mangle)]
pub fn trap_from_kernel() -> ! {
    panic!(
        "[kernel] Trap is not defined in kernel: cause = {:?}, sepc = {:#x}, stval = {:#x}",
        scause::read().cause(),
        riscv::register::sepc::read(),
        stval::read()
    );
}
