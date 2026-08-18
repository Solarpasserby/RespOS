// LoongArch 板级配置。
//
// 默认目标：QEMU loongarch64 `virt` 机器。
// `board_ls2k1000` feature：龙芯 2K1000LA 开发板（全国大学生操作系统大赛）。

// 时钟频率。QEMU virt 的 rdtime.d/CPUCFG 通常对应 100 MHz。
// 2K1000LA 的稳定计数器精确频率需以 cpucfg(4/5)/FDT/手册复核；Stage 3 前保持 100 MHz。
//
// 这组值刻意拆成三类：
// - HARDWARE_CLOCK_FREQ：真实硬件计数器频率，用于 timer interrupt 和 timeout。
// - USER_CLOCK_FREQ：用户可见时间频率，用于 gettimeofday/clock_gettime，可为 bench 调整。
// - ACCOUNTING_CLOCK_FREQ：times()/getrusage() 这类 CPU 时间记账的换算频率。
pub const HARDWARE_CLOCK_FREQ: usize = 100_000_000;
pub const USER_CLOCK_FREQ: usize = 100_000_000;
pub const ACCOUNTING_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;

pub use crate::platform::{
    HIGH_MEMORY_START, LOW_MEMORY_END, MAX_PHYSICAL_MEMORY_END, MEMORY_END, MEMORY_START, MMIO,
    UART_BASE,
};

use core::sync::atomic::{AtomicUsize, Ordering};

static PHYSICAL_MEMORY_END: AtomicUsize = AtomicUsize::new(MEMORY_END);

pub fn physical_memory_end() -> usize {
    PHYSICAL_MEMORY_END.load(Ordering::Acquire)
}

pub fn physical_memory_size() -> usize {
    LOW_MEMORY_END + physical_memory_end().saturating_sub(HIGH_MEMORY_START)
}

pub fn init_physical_memory_end(argc: usize, argv: usize) {
    if let Some(end) = crate::platform::discover_physical_memory_end(argc, argv) {
        PHYSICAL_MEMORY_END.store(end, Ordering::Release);
    }
}

pub const PCI_ECAM_BASE: usize = 0x2000_0000;
pub const PCI_ECAM_SIZE: usize = 0x1000_0000;
pub const PCI_MMIO_BASE: usize = 0x4000_0000;
pub const PCI_MMIO_SIZE: usize = 0x1000_0000;
pub const GED_REG_BASE: usize = 0x100e_0000;
pub const GED_REG_SIZE: usize = 0x1000;
