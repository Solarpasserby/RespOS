// src/sbi.rs

//! ### SBI 模块
//!
//! 调用 SBI 的服务，实现一些更底层的操作，并封装成函数使用

use core::arch::asm;

const SBI_EXT_HSM: usize = 0x4853_4d;
const SBI_HSM_HART_START: usize = 0;
const SBI_EXT_SPI: usize = 0x7350_49;
const SBI_SPI_SEND_IPI: usize = 0;
const SBI_EXT_RFNC: usize = 0x5246_4e43;
const SBI_RFNC_REMOTE_SFENCE_VMA: usize = 1;

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

/// 向终端打印字符
pub fn console_putchar(c: usize) {
    #[allow(deprecated)] // TODO: 被弃用的接口，但是胜在简单，之后可以试着重写
    sbi_rt::legacy::console_putchar(c);
    // let temp = sbi_rt::console_write(bytes) // TODO: 新接口不知道怎么用
    // if temp.error != 0 { panic!("omg") }
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
