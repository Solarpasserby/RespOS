// os/src/trap.rs

//! ### ~~中断~~异常模块
//! 
//! 注：应当注意到目前内核台下触发中断会被屏蔽
//! 因此无需担心某些过程是否需要关闭中断

mod context;

use riscv::register::{
    mtvec::TrapMode,
    stvec, stval, sie,
    scause::{self, Trap, Exception, Interrupt},
};
use core::arch::{ global_asm, asm };
use crate::syscall::*;
use crate::task::{ suspend_current_and_run_next, exit_current_and_run_next };
use crate::timer::set_next_ti_trigger;
use crate::config::{ TRAMPOLINE, TRAP_CONTEXT };
use crate::task::{ current_user_token, with_current_trap_cx };

pub use context::TrapContext;


global_asm!(include_str!("trap/trap.S"));

pub fn init() {
    // __alltraps 被映射到跳板段，该段非恒等映射，无法使用其逻辑地址，转而使用跳板首地址
    // unsafe { stvec::write(__alltraps as *const() as usize, TrapMode::Direct); }

    set_user_trap_entry();
}

/// ~~中断~~异常处理函数
/// 
/// 用户程序上下文保存于内核栈上，包含用户程序使用的寄存器数据以及系统调用传递的寄存器参数
/// 
/// 该函数根据 `CSR` 区分不同异常类型，对不同类型异常做不同处理
#[unsafe(no_mangle)]
pub fn trap_handler() {
    set_kernel_trap_entry();
    let scause = scause::read();
    let stval = stval::read();
    match scause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            with_current_trap_cx(|trap_cx| {
                trap_cx.sepc += 4; // 异常处理完成后直接执行后续指令
                trap_cx.x[10] = syscall(trap_cx.x[17], [trap_cx.x[10], trap_cx.x[11], trap_cx.x[12]]) as usize;
            });
        }
        Trap::Exception(Exception::StoreFault) |
        Trap::Exception(Exception::StorePageFault) | 
        Trap::Exception(Exception::LoadFault) |
        Trap::Exception(Exception::LoadPageFault) => {
            println!("[kernel] PageFault in application, bad addr = {:#x}, kernel killed it.", stval);
            exit_current_and_run_next();
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            println!("[kernel] IllegalInstruction in application, kernel killed it.");
            exit_current_and_run_next();
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_ti_trigger();
            suspend_current_and_run_next();
        }
        _ => {
            panic!("Unsupported trap {:?}, stval = {:#?}!", scause.cause(), stval);
        }
    };
    trap_return();
}

#[unsafe(no_mangle)]
pub fn trap_from_kernel() -> ! {
    panic!("[kernel] Trap is not defined in kernel!");
}

#[unsafe(no_mangle)]
pub fn trap_return() -> ! {
    set_user_trap_entry();
    let trap_cx_ptr = TRAP_CONTEXT;
    let user_satp = current_user_token();
    unsafe extern "C" {
        fn __alltraps();
        fn __restore();
    }
    let restore_va = __restore as *const() as usize - __alltraps as *const() as usize + TRAMPOLINE;
    unsafe {
        asm!(
            "fence.i",
            "jr {restore_va}",
            restore_va = in(reg) restore_va,
            in("a0") trap_cx_ptr,
            in("a1") user_satp,
            options(noreturn)
        );
    }
}

fn set_user_trap_entry() {
    unsafe {
        stvec::write(TRAMPOLINE as usize, TrapMode::Direct);
    }
}


// 内核态不允许发生用户态异常
fn set_kernel_trap_entry() {
    unsafe {
        stvec::write(trap_from_kernel as *const() as usize, TrapMode::Direct);
    }
}

pub fn enable_timer_interrupt() {
    unsafe { sie::set_stimer(); }
}
