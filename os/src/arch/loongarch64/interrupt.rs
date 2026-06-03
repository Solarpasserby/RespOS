// os/src/arch/loongarch64/interrupt.rs
//
// LoongArch 中断状态守卫
// 使用 CSR_CRMD.IE 位（bit 2）替代 RISC-V 的 sstatus.SIE。

/// 读 CRMD 寄存器中的 IE 位
#[inline(always)]
fn read_ie() -> bool {
    super::register::crmd::interrupt_enabled()
}

/// 关中断（清除 CRMD.IE）
#[inline(always)]
fn clear_ie() {
    unsafe {
        super::register::crmd::set_interrupt_enabled(false);
    }
}

/// 开中断（置位 CRMD.IE）
#[inline(always)]
fn set_ie() {
    unsafe {
        super::register::crmd::set_interrupt_enabled(true);
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
