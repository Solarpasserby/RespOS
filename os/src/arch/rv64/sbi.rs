// src/sbi.rs

//! ### SBI 模块
//!
//! 调用 SBI 的服务，实现一些更底层的操作，并封装成函数使用

use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

const SBI_EXT_HSM: usize = 0x4853_4d;
const SBI_HSM_HART_START: usize = 0;
const SBI_EXT_SPI: usize = 0x7350_49;
const SBI_SPI_SEND_IPI: usize = 0;
const SBI_EXT_RFNC: usize = 0x5246_4e43;
const SBI_RFNC_REMOTE_SFENCE_VMA: usize = 1;

// QEMU virt 与 StarFive JH7110 的 UART0 物理基址相同，但寄存器布局不同：
// QEMU 以字节访问、寄存器步进为 1；JH7110 DW8250 以 32 位访问、reg-shift=2。
const UART_BASE: usize = 0x1000_0000;
const LSR_TX_EMPTY: u8 = 1 << 5; // 发送保持寄存器空

/// `mm::init` 激活正式内核页表后置为 true；UART 已进入 direct map，可直写。
static DIRECT_UART_READY: AtomicBool = AtomicBool::new(false);

/// 由 `main::rust_main_high` 在 `mm::init()` 之后调用，切换到直写 UART 路径。
pub fn mark_direct_uart_ready() {
    DIRECT_UART_READY.store(true, Ordering::Release);
}

#[cfg(not(feature = "board_jh7110"))]
unsafe fn direct_uart_putchar(c: u8) {
    let base = crate::config::KERNEL_BASE + UART_BASE;
    let thr = base as *mut u8;
    let lsr = (base + 5) as *const u8;
    unsafe {
        while (core::ptr::read_volatile(lsr) & LSR_TX_EMPTY) == 0 {}
        core::ptr::write_volatile(thr, c);
    }
}

#[cfg(feature = "board_jh7110")]
unsafe fn direct_uart_putchar(c: u8) {
    const UART_REG_SHIFT: usize = 2;
    const UART_THR_REG: usize = 0;
    const UART_LSR_REG: usize = 5;

    let base = crate::config::KERNEL_BASE + UART_BASE;
    let thr = (base + (UART_THR_REG << UART_REG_SHIFT)) as *mut u32;
    let lsr = (base + (UART_LSR_REG << UART_REG_SHIFT)) as *const u32;
    unsafe {
        while (core::ptr::read_volatile(lsr) & u32::from(LSR_TX_EMPTY)) == 0 {}
        core::ptr::write_volatile(thr, u32::from(c));
    }
}

/// 以 SBI v0.2+ HSM 扩展启动一个 hart。
///
/// `start_addr` 必须是关闭分页时可执行的物理地址；次 hart 会从入口汇编
/// 重新开始，携带 `hart_id` 和 `opaque` 进入 `rust_main`。
pub fn hart_start(hart_id: usize, start_addr: usize, opaque: usize) -> Result<(), isize> {
    let error: usize;
    let _value: usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") hart_id => error,
            inlateout("a1") start_addr => _value,
            in("a2") opaque,
            in("a6") SBI_HSM_HART_START,
            in("a7") SBI_EXT_HSM,
            options(nostack)
        );
    }
    if error == 0 {
        Ok(())
    } else {
        Err(error as isize)
    }
}

/// 向 `hart_mask_base` 起的 hart mask 发送 supervisor software interrupt。
///
/// 调用方必须先发布其共享状态；IPI 只承担远端从 WFI 醒来并重新检查状态的职责。
pub fn send_ipi(hart_mask: usize, hart_mask_base: usize) -> Result<(), isize> {
    let error: usize;
    let _value: usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") hart_mask => error,
            inlateout("a1") hart_mask_base => _value,
            in("a6") SBI_SPI_SEND_IPI,
            in("a7") SBI_EXT_SPI,
            options(nostack)
        );
    }
    if error == 0 {
        Ok(())
    } else {
        Err(error as isize)
    }
}

/// 在远端 hart 上执行 `SFENCE.VMA`。
///
/// 早期 SMP 采用 all-address-space 形式，避免当前无 ASID 分配器时错误地
/// 将一个地址空间的旧 translation 留在远端 TLB。调用者应先完成 PTE 写入
/// 并在本 hart 执行本地 fence。
pub fn remote_sfence_vma(hart_mask: usize, hart_mask_base: usize) -> Result<(), isize> {
    let error: usize;
    let _value: usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") hart_mask => error,
            inlateout("a1") hart_mask_base => _value,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a6") SBI_RFNC_REMOTE_SFENCE_VMA,
            in("a7") SBI_EXT_RFNC,
            options(nostack)
        );
    }
    if error == 0 {
        Ok(())
    } else {
        Err(error as isize)
    }
}

/// 设置 mtimecmp ，使指定时钟周期产生时钟中断
pub fn set_timer(time_value: usize) {
    sbi_rt::set_timer(time_value as _);
}

/// 向终端打印一个字节。
///
/// 正式内核页表激活（`mm::init` 完成、`mark_direct_uart_ready` 被调用）之后，板级
/// UART 通过 direct map 可达，按对应寄存器布局直接写入，保证多字节 UTF-8 原样输出。
/// 此前（早期启动阶段，UART 尚未映射）回退到 SBI legacy 接口；该接口只保证 ASCII
/// 正确，但早期启动本就不输出非 ASCII 内容。
pub fn console_putchar(c: usize) {
    if DIRECT_UART_READY.load(Ordering::Acquire) {
        unsafe {
            direct_uart_putchar(c as u8);
        }
    } else {
        #[allow(deprecated)] // 早期启动阶段 UART 未映射，只能走 SBI
        sbi_rt::legacy::console_putchar(c);
    }
}

/// 向终端打印字符
pub fn console_getchar() -> usize {
    #[allow(deprecated)]
    sbi_rt::legacy::console_getchar()
}

/// 关闭机器
pub fn shutdown(failure: bool) -> ! {
    use sbi_rt::{NoReason, Shutdown, SystemFailure, system_reset};
    if !failure {
        system_reset(Shutdown, NoReason);
    } else {
        system_reset(Shutdown, SystemFailure);
    }
    unreachable!()
}

/// Reset the virtual machine while preserving device state that survives a
/// platform reset, such as the RTC's programmed offset.
pub fn restart() -> ! {
    use sbi_rt::{ColdReboot, NoReason, system_reset};
    system_reset(ColdReboot, NoReason);
    unreachable!()
}
