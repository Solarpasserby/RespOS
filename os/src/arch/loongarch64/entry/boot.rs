// os/src/arch/loongarch64/entry/boot.rs

use core::arch::asm;

/// 从汇编 _start 跳转到 rust_main 的桥接函数
///
/// 初始阶段 phys == virt，直接跳转即可。
#[unsafe(no_mangle)]
pub fn enter_main() {
    unsafe {
        asm!(
            "la.local   $t0, rust_main",
            "jirl       $zero, $t0, 0",
            options(noreturn)
        );
    }
}

/// 从 QEMU secondary mailbox 物理入口跳转到 Rust 次核启动路径。
#[unsafe(no_mangle)]
pub fn enter_secondary() {
    unsafe {
        asm!(
            "la.local   $t0, rust_secondary_main",
            "jirl       $zero, $t0, 0",
            options(noreturn)
        );
    }
}
