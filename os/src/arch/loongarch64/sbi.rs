// os/src/arch/loongarch64/sbi.rs
//
// LoongArch 的 "SBI" 等效层 —— 直接操作硬件
// RISC-V 通过 SBI ecall 调用固件服务，LoongArch 因为直接运行在裸机（-bios），
// 所以这里直接访问 UART，并通过本地 register 模块操作 CSR。

use super::register;

const UART_BASE: usize = 0x1fe0_01e0;
// NS16550 寄存器偏移
const UART_THR: usize = UART_BASE + 0; // Transmit Holding Register
const UART_RBR: usize = UART_BASE + 0; // Receiver Buffer Register
const UART_LSR: usize = UART_BASE + 5; // Line Status Register
const LSR_RX_READY: u8 = 1 << 0; // Data Ready
const LSR_TX_EMPTY: u8 = 1 << 5; // Transmitter Holding Register Empty

/// 向控制台打印一个字符
pub fn console_putchar(c: usize) {
    unsafe {
        // 等待发送寄存器为空
        while (core::ptr::read_volatile(UART_LSR as *const u8) & LSR_TX_EMPTY) == 0 {}
        core::ptr::write_volatile(UART_THR as *mut u8, c as u8);
    }
}

/// 从控制台读取一个字符（无数据时返回 0）
pub fn console_getchar() -> usize {
    unsafe {
        if (core::ptr::read_volatile(UART_LSR as *const u8) & LSR_RX_READY) != 0 {
            core::ptr::read_volatile(UART_RBR as *const u8) as usize
        } else {
            0
        }
    }
}

/// 设置定时器，在经过指定 tick 数后产生时钟中断
pub fn set_timer(deadline: usize) {
    unsafe {
        let now = register::timer::read_time();
        register::timer::set_oneshot(deadline.saturating_sub(now));
    }
}

/// 清除定时器中断标志
pub fn clear_timer_interrupt() {
    unsafe {
        register::timer::clear_interrupt();
    }
}

/// 关闭机器
pub fn shutdown(_failure: bool) -> ! {
    // QEMU loongarch64 virt: 通过 ACPI PM1a 寄存器触发关机
    // 写 SLP_TYP_S5 | SLP_EN 到 PM1a_CNT
    unsafe {
        core::ptr::write_volatile(0x1000_0000 as *mut u16, 0x3c00u16);
    }
    register::idle()
}
