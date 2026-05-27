// os/src/arch/loongarch64/trap/mod.rs

mod context;

use super::timer::set_next_ti_trigger;
use crate::syscall::*;
use crate::task::{exit_and_run_next, handle_signals, yield_current_task};
use core::arch::asm;
use core::arch::global_asm;

pub use context::TrapContext;

/// 页错误原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultCause {
    Instruction,
    Load,
    Store,
}

global_asm!(include_str!("trap.S"));

unsafe extern "C" {
    fn __trap_from_user();
    fn __trap_from_kernel();
    pub fn __restore() -> !;
}

// LoongArch CSR 常量
const CSR_EENTRY: usize = 0xC; // 异常入口地址
const CSR_ECFG: usize = 0x4;   // 异常配置
const CSR_ESTAT: usize = 0x5;  // 异常状态
const CSR_ERA: usize = 0x6;    // 异常返回地址
const CSR_BADV: usize = 0x7;   // 坏地址

// ESTAT 位定义
const ESTAT_ECODE_MASK: usize = 0xFFFF; // 异常码 [15:0]
const ESTAT_IS_SHIFT: usize = 11;       // 中断标志位

// LoongArch 异常码
const ECODE_SYS: usize = 3;   // syscall
const ECODE_PIF: usize = 8;   // Page Instruction Fault
const ECODE_PIS: usize = 9;   // Page Invalid Store
const ECODE_PIL: usize = 10;  // Page Invalid Load
const ECODE_PPI: usize = 11;  // Page Protection
const ECODE_INE: usize = 5;   // Illegal Instruction

// LoongArch 中断号 (ESTAT[12:10] 或中断编码)
const INT_TI: usize = 11;     // Timer Interrupt

/// 读写 CSR 的辅助宏，使用 `const` 确保 CSR 编号为立即数
macro_rules! csr_read {
    ($csr:ident) => {{
        let val: usize;
        unsafe {
            asm!("csrrd {}, {}", out(reg) val, const $csr);
        }
        val
    }};
}

macro_rules! csr_write {
    ($csr:ident, $val:expr) => {
        unsafe {
            asm!("csrwr {}, {}", in(reg) $val, const $csr);
        }
    };
}

macro_rules! csr_set_bits {
    ($csr:ident, $bits:expr) => {{
        unsafe {
            let mut val: usize;
            asm!("csrrd {}, {}", out(reg) val, const $csr);
            val |= $bits;
            asm!("csrwr {}, {}", in(reg) val, const $csr);
        }
    }};
}

pub fn init() {
    // 设置异常入口点为 __trap_from_kernel（内核初始化期间）
    csr_write!(CSR_EENTRY, __trap_from_kernel as usize);
}

pub fn enable_timer_interrupt() {
    // ECFG.LIE[11] = 1: 使能定时器中断
    csr_set_bits!(CSR_ECFG, 1 << 11);
}

/// 异常处理入口
#[unsafe(no_mangle)]
pub fn trap_handler(cx: &mut TrapContext) {
    let estat = csr_read!(CSR_ESTAT);
    let is_interrupt = (estat >> ESTAT_IS_SHIFT) & 1 != 0;
    let ecode = estat & ESTAT_ECODE_MASK;

    if is_interrupt {
        // 中断处理
        match ecode {
            INT_TI => {
                // 定时器中断
                set_next_ti_trigger();
                yield_current_task();
            }
            _ => {
                panic!(
                    "[kernel] Unsupported interrupt: ecode = {:#x}, era = {:#x}",
                    ecode, cx.era
                );
            }
        }
    } else {
        // 异常处理
        match ecode {
            ECODE_SYS => {
                // 系统调用: syscall 指令会自动将下一条指令地址存入 ERA
                // 返回后应当执行下一条指令，ERA 已经是正确的
                let id = cx.syscall_id();
                let args = cx.syscall_args();
                cx.x[4] = match syscall(id, args) {
                    Ok(ret) => ret,
                    Err(err) => err.as_ret() as usize,
                };
            }
            ECODE_PIF | ECODE_PIL | ECODE_PIS | ECODE_PPI => {
                let badv = csr_read!(CSR_BADV);
                println!(
                    "[kernel] PageFault in application, ecode = {:#x}, era = {:#x}, bad addr = {:#x}, kernel killed it.",
                    ecode, cx.era, badv
                );
                exit_and_run_next(-2);
            }
            ECODE_INE => {
                println!("[kernel] IllegalInstruction in application, kernel killed it.");
                exit_and_run_next(-3);
            }
            _ => {
                let badv = csr_read!(CSR_BADV);
                panic!(
                    "Unsupported trap: ecode = {:#x}, is_int = {}, era = {:#x}, badv = {:#x}!",
                    ecode, is_interrupt, cx.era, badv
                );
            }
        }
    }
    handle_signals();
}

#[unsafe(no_mangle)]
pub fn trap_from_kernel() -> ! {
    let estat = csr_read!(CSR_ESTAT);
    let era = csr_read!(CSR_ERA);
    let badv = csr_read!(CSR_BADV);
    panic!(
        "[kernel] Trap is not defined in kernel: estat = {:#x}, era = {:#x}, badv = {:#x}",
        estat, era, badv
    );
}
