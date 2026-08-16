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

// —— 物理内存布局 ——
//
// QEMU loongarch64 virt keeps 256 MiB of low RAM and places the remainder above
// the PCI/MMIO hole.  `MEMORY_END` is only the compatibility fallback used when
// QEMU memory discovery is unavailable.
//
// 2K1000LA 的 DDR 布局尚未接入：Stage 1 只做 entry + early UART，不触碰 frame
// allocator / direct map / heap，因此下面仍是 QEMU 占位值。Stage 2 依据板载 U-Boot
// `bdinfo` / `fdt print /memory` + 龙芯 2K1000LA 处理器用户手册确定 DDR 起止/是否分段后替换。
// 已知：DDR 起始物理地址 = 0x0020_0000（StarryOS someboot 内核装载点即 DDR 起点）。
pub const MEMORY_START: usize = 0;
pub const LOW_MEMORY_END: usize = 0x1000_0000;
pub const HIGH_MEMORY_START: usize = 0x8000_0000;
pub const MEMORY_END: usize = 0x3_7000_0000;
/// End of the high RAM window for the contest's supported 36 GiB maximum.
pub const MAX_PHYSICAL_MEMORY_END: usize = 0x9_7000_0000;

// —— 2K1000LA 固定外设地址（Stage 2 起逐步接入，这里先留档）——
//   UART0    (NS16550 兼容): 0x1fe0_01e0  —— 与 QEMU virt 相同（QEMU 即仿 Loongson）
//   LIOINTC  主中断控制器:    0x1fe0_1400（reg）/ 0x1fe0_1040（ISR）
//   GMAC0    以太网:          0x4004_0000
//   SATA/AHCI: 由 DTB compatible "loongson,ls2k1000-ahci" 的 reg 给定

/// QEMU loongarch64/virt fw_cfg MMIO window.
#[cfg(not(feature = "board_ls2k1000"))]
const FW_CFG_DATA: usize = 0x1e02_0000;
#[cfg(not(feature = "board_ls2k1000"))]
const FW_CFG_SELECTOR: usize = FW_CFG_DATA + 8;
#[cfg(not(feature = "board_ls2k1000"))]
const FW_CFG_RAM_SIZE: u16 = 0x0003;

use core::sync::atomic::{AtomicUsize, Ordering};

static PHYSICAL_MEMORY_END: AtomicUsize = AtomicUsize::new(MEMORY_END);

pub fn physical_memory_end() -> usize {
    PHYSICAL_MEMORY_END.load(Ordering::Acquire)
}

pub fn physical_memory_size() -> usize {
    LOW_MEMORY_END + physical_memory_end().saturating_sub(HIGH_MEMORY_START)
}

/// Discover RAM through QEMU's architecture-neutral fw_cfg item. This runs
/// while DMW0 still provides physical-address MMIO access. Invalid or
/// unsupported input deliberately leaves the compatibility fallback in place.
#[cfg(not(feature = "board_ls2k1000"))]
pub fn init_physical_memory_end() {
    if let Some(end) = unsafe { fw_cfg_memory_end() } {
        let minimum = HIGH_MEMORY_START + crate::config::KERNEL_HEAP_SIZE;
        PHYSICAL_MEMORY_END.store(
            end.clamp(minimum, MAX_PHYSICAL_MEMORY_END),
            Ordering::Release,
        );
    }
}

/// 2K1000LA 没有 QEMU fw_cfg。Stage 1 保持 `MEMORY_END` fallback；
/// Stage 2 改为从 U-Boot 传入的 DTB `/memory` reg 解析实际末址。
#[cfg(feature = "board_ls2k1000")]
pub fn init_physical_memory_end() {}

#[cfg(not(feature = "board_ls2k1000"))]
unsafe fn fw_cfg_memory_end() -> Option<usize> {
    unsafe {
        // The MMIO transport uses a big-endian selector and permits a single
        // 64-bit data read. QEMU returns the selected bytes in address order.
        core::ptr::write_volatile(FW_CFG_SELECTOR as *mut u16, FW_CFG_RAM_SIZE.to_be());
        let ram_size = u64::from_le(core::ptr::read_volatile(FW_CFG_DATA as *const u64));
        if ram_size < LOW_MEMORY_END as u64 {
            return None;
        }
        let high_size = ram_size.checked_sub(LOW_MEMORY_END as u64)?;
        usize::try_from((HIGH_MEMORY_START as u64).checked_add(high_size)?).ok()
    }
}

pub const PCI_ECAM_BASE: usize = 0x2000_0000;
pub const PCI_ECAM_SIZE: usize = 0x1000_0000;
pub const PCI_MMIO_BASE: usize = 0x4000_0000;
pub const PCI_MMIO_SIZE: usize = 0x1000_0000;
pub const GED_REG_BASE: usize = 0x100e_0000;
pub const GED_REG_SIZE: usize = 0x1000;

// MMIO 设备地址区间 (QEMU loongarch64 virt 平台)
pub const MMIO: &[(usize, usize)] = &[
    (0x1000_1000, 0x00_1000),       // Virtio Block
    (GED_REG_BASE, GED_REG_SIZE),   // ACPI GED power/reset registers
    (0x1fe0_0000, 0x00_1000),       // UART
    (0x0010_0000, 0x00_2000),       // VIRT_TEST/RTC
    (PCI_ECAM_BASE, PCI_ECAM_SIZE), // PCIe ECAM
    (PCI_MMIO_BASE, PCI_MMIO_SIZE), // PCI BAR memory window
];
