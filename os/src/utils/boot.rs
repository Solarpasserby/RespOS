// os/src/utils/boot.rs

use core::arch::asm;
use crate::config::KERNEL_BASE;

#[unsafe(no_mangle)]
pub fn enter_main() {
    unsafe { // 调整栈指针 加上偏移，跳转到 rust_main
        asm!(
            "add sp, sp, {offset}",
            "la t0, rust_main",
            "add t0, t0, {offset}",
            "jalr zero, 0(t0)",
            offset = in(reg) KERNEL_BASE,
            options(noreturn)
        );
    }
}
