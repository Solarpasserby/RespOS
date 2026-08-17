// os/src/arch/loongarch64/sbi.rs
//
// LoongArch 的 "SBI" 等效层 —— 直接操作硬件
// RISC-V 通过 SBI ecall 调用固件服务，LoongArch 因为直接运行在裸机（-bios），
// 所以这里直接访问 UART，并通过本地 register 模块操作 CSR。

use super::{
    config::{GED_REG_BASE, UART_BASE},
    register,
};

// NS16550 寄存器偏移（byte stride，THR@0 / LSR@5）
const UART_THR: usize = UART_BASE + 0; // Transmit Holding Register
const UART_RBR: usize = UART_BASE + 0; // Receiver Buffer Register
const UART_LSR: usize = UART_BASE + 5; // Line Status Register
const LSR_RX_READY: u8 = 1 << 0; // Data Ready
const LSR_TX_EMPTY: u8 = 1 << 5; // Transmitter Holding Register Empty
const ACPI_GED_REG_BASE: usize = GED_REG_BASE + 0x1c;
const ACPI_GED_REG_SLEEP_CTL: usize = ACPI_GED_REG_BASE;
const ACPI_GED_REG_RESET: usize = ACPI_GED_REG_BASE + 2;
const ACPI_GED_SLP_TYP_S5: u8 = 0x05;
const ACPI_GED_SLP_TYP_SHIFT: u8 = 2;
const ACPI_GED_SLP_EN: u8 = 0x20;
const ACPI_GED_RESET_VALUE: u8 = 0x42;

#[inline]
fn mmio_addr(addr: usize) -> usize {
    if super::paging_enabled() && !super::low_direct_map_enabled() {
        addr + crate::config::KERNEL_BASE
    } else {
        addr
    }
}

/// 向控制台打印一个字符
pub fn console_putchar(c: usize) {
    unsafe {
        let thr = mmio_addr(UART_THR);
        let lsr = mmio_addr(UART_LSR);
        // 等待发送寄存器为空
        while (core::ptr::read_volatile(lsr as *const u8) & LSR_TX_EMPTY) == 0 {}
        core::ptr::write_volatile(thr as *mut u8, c as u8);
    }
}

/// Stage-1 早期控制台：只写 NS16550 THR，不依赖锁、格式化或 MMU 状态。
///
/// 仅在进入完整 driver framework / MMU 之前使用；此时 entry 建立的 DMW0 identity
/// 使物理地址可直接访问。UART_BASE 由板级配置决定：QEMU virt 0x1fe0_01e0 /
/// 2K1000LA 0x1fe2_0000（byte stride，THR@0 / LSR@5）。
#[cfg(feature = "board_ls2k1000")]
pub fn early_putchar(c: u8) {
    unsafe {
        // 等待发送寄存器为空，再写入一个字节。
        while (core::ptr::read_volatile(UART_LSR as *const u8) & LSR_TX_EMPTY) == 0 {}
        core::ptr::write_volatile(UART_THR as *mut u8, c);
    }
}

/// 逐字节输出字符串（Stage-1 启动诊断用，不解释 UTF-8）。
#[cfg(feature = "board_ls2k1000")]
pub fn early_print(s: &str) {
    for &b in s.as_bytes() {
        early_putchar(b);
    }
}

/// 以十六进制输出一个 usize（Stage-2 诊断用，打印 DDR 末址等）。
#[cfg(feature = "board_ls2k1000")]
pub fn early_print_hex(v: usize) {
    early_print("0x");
    for shift in (0..64).rev().step_by(4) {
        let d = ((v >> shift) & 0xf) as u8;
        let c = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        early_putchar(c);
    }
}

/// 从控制台读取一个字符（无数据时返回 0）
pub fn console_getchar() -> usize {
    unsafe {
        let rbr = mmio_addr(UART_RBR);
        let lsr = mmio_addr(UART_LSR);
        if (core::ptr::read_volatile(lsr as *const u8) & LSR_RX_READY) != 0 {
            core::ptr::read_volatile(rbr as *const u8) as usize
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
pub fn shutdown(failure: bool) -> ! {
    unsafe {
        if failure {
            core::ptr::write_volatile(
                mmio_addr(ACPI_GED_REG_RESET) as *mut u8,
                ACPI_GED_RESET_VALUE,
            );
        } else {
            let s5_poweroff = ACPI_GED_SLP_EN | (ACPI_GED_SLP_TYP_S5 << ACPI_GED_SLP_TYP_SHIFT);
            core::ptr::write_volatile(mmio_addr(ACPI_GED_REG_SLEEP_CTL) as *mut u8, s5_poweroff);
        }
    }
    register::idle()
}

/// Reset the LoongArch virtual platform through the ACPI GED reset register.
pub fn restart() -> ! {
    unsafe {
        core::ptr::write_volatile(
            mmio_addr(ACPI_GED_REG_RESET) as *mut u8,
            ACPI_GED_RESET_VALUE,
        );
    }
    register::idle()
}
