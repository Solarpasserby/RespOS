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
// 2K1000LA（LSGD2K10 板）DDR 为两段（与 QEMU 同构，仅常量不同），来自真机
// Linux 启动日志 `Early memory node ranges`：
//   node 0: [mem 0x0000000000200000-0x000000000affffff]  （低段 174 MiB）
//   node 0: [mem 0x0000000090000000-0x00000000bfffffff]  （高段 768 MiB）
#[cfg(not(feature = "board_ls2k1000"))]
pub const MEMORY_START: usize = 0;
#[cfg(feature = "board_ls2k1000")]
pub const MEMORY_START: usize = 0x0020_0000;

#[cfg(not(feature = "board_ls2k1000"))]
pub const LOW_MEMORY_END: usize = 0x1000_0000;
#[cfg(feature = "board_ls2k1000")]
pub const LOW_MEMORY_END: usize = 0x0b00_0000;

#[cfg(not(feature = "board_ls2k1000"))]
pub const HIGH_MEMORY_START: usize = 0x8000_0000;
#[cfg(feature = "board_ls2k1000")]
pub const HIGH_MEMORY_START: usize = 0x9000_0000;

#[cfg(not(feature = "board_ls2k1000"))]
pub const MEMORY_END: usize = 0x3_7000_0000;
#[cfg(feature = "board_ls2k1000")]
pub const MEMORY_END: usize = 0xc000_0000;

/// End of the high RAM window for the supported maximum.
#[cfg(not(feature = "board_ls2k1000"))]
pub const MAX_PHYSICAL_MEMORY_END: usize = 0x9_7000_0000;
#[cfg(feature = "board_ls2k1000")]
pub const MAX_PHYSICAL_MEMORY_END: usize = 0xc000_0000;

// —— 2K1000LA 固定外设地址（来自真机 Linux 启动日志 / 设备树，Stage 2 起逐步接入）——
//   UART console: 0x1fe2_0000（ttyS0，16550A，irq 16）—— 不是 QEMU 的 0x1fe0_01e0
//   LIOINTC:      0x1fe0_1400（interrupt-controller@1fe01400）
//   SATA/AHCI:    0x400e_0000（irq 19）
//   GMAC:         eth0 0x4004_0000 / eth1 0x4005_0000

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
pub fn init_physical_memory_end(_argc: usize, _argv: usize) {
    if let Some(end) = unsafe { fw_cfg_memory_end() } {
        let minimum = HIGH_MEMORY_START + crate::config::KERNEL_HEAP_SIZE;
        PHYSICAL_MEMORY_END.store(
            end.clamp(minimum, MAX_PHYSICAL_MEMORY_END),
            Ordering::Release,
        );
    }
}

/// 2K1000LA 没有 QEMU fw_cfg；从 U-Boot `go` 传入的 DTB `/memory` reg 解析实际
/// DDR 末址。解析失败时保持 `MEMORY_END` fallback。
#[cfg(feature = "board_ls2k1000")]
pub fn init_physical_memory_end(argc: usize, argv: usize) {
    // 2K1000LA DDR 起始物理地址（StarryOS someboot 内核装载点）。
    const DDR_BASE: usize = 0x0020_0000;

    let Some(fdt_addr) = crate::arch::fdt::fdt_addr_from_boot_args(argc, argv) else {
        return;
    };
    let Some(end) = (unsafe { crate::arch::fdt::memory_end_from_fdt(fdt_addr) }) else {
        return;
    };
    // 至少覆盖 DDR 基址，且不超过 48-bit 物理地址上限。
    if end > DDR_BASE && end <= (1usize << 48) {
        PHYSICAL_MEMORY_END.store(end, Ordering::Release);
    }
}

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
