//! Loongson 2K1000LA LIOINTC（Loongson IO Interrupt Controller）2.0 最小初始化。
//!
//! 时钟中断是 LoongArch 核内中断（ESTAT bit11），**不经过 LIOINTC**；LIOINTC 只接
//! 外部设备中断（UART/AHCI/GMAC）。Stage 3 只做最小初始化（全部屏蔽），Stage 4 起
//! 按需使能具体 IRQ 位并配置 parent 路由。
//!
//! 寄存器布局参考 Linux `drivers/irqchip/irq-loongson-liointc.c`：
//!   LIOINTC_INTC_CHIP_START = 0x20
//!   INT_EN_STATUS = 0x24（enabled status）
//!   INT_ENABLE    = 0x28
//!   INT_DISABLE   = 0x2c
//!   INT_POL       = 0x30（仅 Loongson-2K 有效）
//!   INT_EDGE      = 0x34
//! 设备树（2K1000LA）：
//!   liointc0: interrupt-controller@1fe01400 { reg = main(0x40) + isr0(0x1fe01040) + isr1(0x1fe01140) }
//!   liointc1: interrupt-controller@1fe01440
//!   liointc0 → cpuintc INT2；liointc1 → cpuintc INT3。

// 真机 MMIO 必须经 uncached DMW0 窗口（VSEG=0x8000）。
const UNCACHE_BASE: usize = 0x8000_0000_0000_0000;

const LIOINTC0_BASE: usize = 0x1fe0_1400;
const LIOINTC1_BASE: usize = 0x1fe0_1440;

const REG_INT_DISABLE: usize = 0x2c;
const REG_INT_EDGE: usize = 0x34;
// Stage 4 接入外部设备中断时使用（当前最小初始化只屏蔽，暂未用到）。
#[allow(dead_code)]
const REG_INT_ENABLE: usize = 0x28;
#[allow(dead_code)]
const REG_INT_POL: usize = 0x30;

#[inline]
fn write_reg(base: usize, off: usize, val: u32) {
    unsafe {
        core::ptr::write_volatile((UNCACHE_BASE | (base + off)) as *mut u32, val);
    }
}

/// 最小初始化：禁用全部 LIOINTC 中断、清边沿触发配置（全部按电平触发）。
///
/// 与 Linux `liointc_init` 一致：`INT_DISABLE=0xffff_ffff`、`INT_EDGE=0`。
/// Stage 4 接入外部设备中断时，再按设备极性写 `INT_POL`/`INT_EDGE` 并置 `INT_ENABLE`
/// 对应位，同时配好 `parent_int_map`（isr0/isr1）把 LIOINTC 输出路由到目标核。
pub fn init() {
    for base in [LIOINTC0_BASE, LIOINTC1_BASE] {
        write_reg(base, REG_INT_DISABLE, 0xffff_ffff);
        write_reg(base, REG_INT_EDGE, 0x0);
    }
}
