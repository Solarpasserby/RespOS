// os/src/arch/loongarch64/sbi.rs
//
// LoongArch 的 "SBI" 等效层 —— 直接操作硬件
// RISC-V 通过 SBI ecall 调用固件服务，LoongArch 因为直接运行在裸机（-bios），
// 所以这里直接访问 UART、CSR 等硬件寄存器。

use core::arch::asm;

const UART_BASE: usize = 0x1fe0_01e0;
// NS16550 寄存器偏移
const UART_THR: usize = UART_BASE + 0; // Transmit Holding Register
const UART_RBR: usize = UART_BASE + 0; // Receiver Buffer Register
const UART_LSR: usize = UART_BASE + 5; // Line Status Register
const LSR_RX_READY: u8 = 1 << 0;       // Data Ready
const LSR_TX_EMPTY: u8 = 1 << 5;       // Transmitter Holding Register Empty

// ---------------------------------------------------------------------------
// LoongArch CSR 编号
// ---------------------------------------------------------------------------
// 定时器相关
const CSR_TCFG: usize = 0x41;  // 定时器配置寄存器
const CSR_TVAL: usize = 0x42;  // 定时器值寄存器（写入比较值）
const CSR_TICLR: usize = 0x44; // 定时器中断清除寄存器
#[allow(dead_code)]
const CSR_CRMD: usize = 0x0;   // 当前模式寄存器（IE=bit2）

// 定时器配置位
#[allow(dead_code)]
const TCFG_EN: usize = 1 << 0;  // 定时器使能
#[allow(dead_code)]
const TCFG_PERIODIC: usize = 1 << 1; // 周期模式

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
pub fn set_timer(time_value: usize) {
    unsafe {
        // 配置定时器：使能 + 单次触发模式
        asm!(
            "csrwr {tmp}, {tcfg}",
            tmp = in(reg) TCFG_EN,
            tcfg = const CSR_TCFG,
        );
        // 写入比较值
        asm!(
            "csrwr {val}, {tval}",
            val = in(reg) time_value,
            tval = const CSR_TVAL,
        );
    }
}

/// 清除定时器中断标志
pub fn clear_timer_interrupt() {
    unsafe {
        // 写 0 到 TICLR 清除中断
        asm!(
            "csrwr {zero}, {ticlr}",
            zero = in(reg) 0usize,
            ticlr = const CSR_TICLR,
        );
    }
}

/// 关闭机器
pub fn shutdown(_failure: bool) -> ! {
    // QEMU loongarch64 virt: 通过 ACPI PM1a 寄存器触发关机
    // 写 SLP_TYP_S5 | SLP_EN 到 PM1a_CNT
    unsafe {
        core::ptr::write_volatile(0x1000_0000 as *mut u16, 0x3c00u16);
    }
    loop {
        unsafe { asm!("idle 0", options(nomem, nostack)) }
    }
}
