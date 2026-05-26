// os/src/arch/loongarch64/interrupt.rs
//
// LoongArch 中断状态守卫
// 使用 CSR_CRMD.IE 位（bit 2）替代 RISC-V 的 sstatus.SIE。

use core::arch::asm;

const CSR_CRMD: usize = 0x0;

// CRMD 寄存器位
const CRMD_IE: usize = 1 << 2; // 中断使能位

/// 读 CRMD 寄存器中的 IE 位
#[inline(always)]
fn read_ie() -> bool {
    let crmd: usize;
    unsafe {
        asm!("csrrd {}, {}", out(reg) crmd, const CSR_CRMD);
    }
    crmd & CRMD_IE != 0
}

/// 关中断（清除 CRMD.IE）
#[inline(always)]
fn clear_ie() {
    unsafe {
        asm!(
            "csrrd {tmp}, {crmd}",
            "bstrins.d {tmp}, {zero}, 3, 2",
            "csrwr {tmp}, {crmd}",
            tmp = out(reg) _,
            zero = in(reg) 0usize,
            crmd = const CSR_CRMD,
        );
    }
}

/// 开中断（置位 CRMD.IE）
#[inline(always)]
fn set_ie() {
    unsafe {
        asm!(
            "csrrd {tmp}, {crmd}",
            "ori {tmp}, {tmp}, 0x4",
            "csrwr {tmp}, {crmd}",
            tmp = out(reg) _,
            crmd = const CSR_CRMD,
        );
    }
}

pub struct InterruptGuard {
    ie_before_lock: bool,
}

impl InterruptGuard {
    #[inline(always)]
    pub fn new() -> Self {
        let ie_before_lock = read_ie();
        if ie_before_lock {
            clear_ie();
        }
        Self { ie_before_lock }
    }
}

impl Drop for InterruptGuard {
    #[inline(always)]
    fn drop(&mut self) {
        if self.ie_before_lock {
            set_ie();
        }
    }
}
