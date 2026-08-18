// LoongArch QEMU virt 机器板级配置

// LoongArch QEMU virt 机器时钟频率。
//
// 这组值刻意拆成三类：
// - HARDWARE_CLOCK_FREQ：真实硬件计数器频率，用于 timer interrupt 和 timeout。
// - USER_CLOCK_FREQ：用户可见时间频率，用于 gettimeofday/clock_gettime，可为 bench 调整。
// - ACCOUNTING_CLOCK_FREQ：times()/getrusage() 这类 CPU 时间记账的换算频率。
//
// 当前 QEMU virt 的 rdtime.d/CPUCFG 通常对应 100MHz。USER_CLOCK_FREQ 保持较低值
// 是为了保留 bench-facing wall clock 的可调空间；不要再把它用于硬件 timer 编程。
pub const HARDWARE_CLOCK_FREQ: usize = 100_000_000;
pub const USER_CLOCK_FREQ: usize = 100_000_000;
pub const ACCOUNTING_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
// QEMU loongarch64 virt keeps 256 MiB of low RAM and places the remainder above
// the PCI/MMIO hole.  `MEMORY_END` is only the compatibility fallback used when
// QEMU memory discovery is unavailable.
pub const MEMORY_START: usize = 0;
pub const LOW_MEMORY_END: usize = 0x1000_0000;
pub const HIGH_MEMORY_START: usize = 0x8000_0000;
pub const MEMORY_END: usize = 0x3_7000_0000;
/// End of the high RAM window for the contest's supported 36 GiB maximum.
pub const MAX_PHYSICAL_MEMORY_END: usize = 0x9_7000_0000;
/// LoongArch QEMU and LS2K1000 currently expose the root filesystem as a
/// whole-disk ext4 device rather than a partition offset.
pub const ROOT_DISK_BASE_BLOCK: usize = 0;
/// QEMU loongarch64/virt fw_cfg MMIO window.
const FW_CFG_DATA: usize = 0x1e02_0000;
const FW_CFG_SELECTOR: usize = FW_CFG_DATA + 8;
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
pub fn init_physical_memory_end() {
    if let Some(end) = unsafe { fw_cfg_memory_end() } {
        let minimum = HIGH_MEMORY_START + crate::config::KERNEL_HEAP_SIZE;
        PHYSICAL_MEMORY_END.store(
            end.clamp(minimum, MAX_PHYSICAL_MEMORY_END),
            Ordering::Release,
        );
    }
}

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
    (0x100d_0000, 0x00_1000),       // LS7A RTC
    (GED_REG_BASE, GED_REG_SIZE),   // ACPI GED power/reset registers
    (0x1fe0_0000, 0x00_1000),       // UART
    (0x0010_0000, 0x00_2000),       // VIRT_TEST/RTC
    (PCI_ECAM_BASE, PCI_ECAM_SIZE), // PCIe ECAM
    (PCI_MMIO_BASE, PCI_MMIO_SIZE), // PCI BAR memory window
];
