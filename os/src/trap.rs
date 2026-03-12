mod context;

use riscv::register::{
    mtvec::TrapMode,
    stvec, stval,
    scause::{self, Trap, Exception},
};
use core::arch::global_asm;
use crate::syscall::*;

pub use context::TrapContext;

global_asm!(include_str!("trap/trap.S"));

pub fn init(){
    unsafe extern "C" {
        fn __alltraps();
    }
    unsafe {
        stvec::write(__alltraps as *const() as usize, TrapMode::Direct);
    }
}

/// ~~中断~~异常处理函数
/// 
/// 用户程序上下文保存于内核栈上，包含用户程序使用的寄存器数据以及系统调用传递的寄存器参数
/// 
/// 该函数根据 `CSR` 区分不同异常类型，对不同类型异常做不同处理
#[unsafe(no_mangle)]
pub fn trap_handler(context: &mut TrapContext) -> &mut TrapContext {
    let scause = scause::read();
    let stval = stval::read();
    match scause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            context.sepc += 4; // 异常处理完成后直接执行后续指令
            context.x[10] = syscall(context.x[17], [context.x[10], context.x[11], context.x[12]]) as usize;
        }
        Trap::Exception(Exception::StoreFault) |
        Trap::Exception(Exception::StorePageFault) => {
            println!("[kernel] PageFault in application, kernel killed it.");
            panic!("[kernel] Cannot continue!");
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            println!("[kernel] IllegalInstruction in application, kernel killed it.");
            panic!("[kernel] Cannot continue!");
        }
        _ => {
            panic!("Unsupported trap {:?}, stval = {:?}!", scause.cause(), stval);
        }
    };
    context
}
