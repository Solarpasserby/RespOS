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
// QEMU loongarch64 virt with `-m 128M` maps RAM in low memory.
pub const MEMORY_START: usize = 0;
pub const MEMORY_END: usize = 0x0800_0000;

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
