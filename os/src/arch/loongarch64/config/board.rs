// LoongArch QEMU virt 机器板级配置

// LoongArch QEMU virt 机器默认时钟频率。
//
// 这组值刻意拆成两类：
// - DEFAULT_CLOCK_FREQ：用户可见时间的换算频率，影响 gettimeofday、
//   clock_gettime 以及 bench 类测例输出。
// - TIMEOUT_CLOCK_FREQ：内核等待/超时使用的硬件计数器频率，影响
//   nanosleep、sigtimedwait、pselect 等相对 timeout 的真实等待时长。
//
// CPUCFG 在 QEMU virt 上通常能读到 100MHz。直接把它用于所有时间路径会
// 同时改变用户时间和 tick 编程，容易让不同测例互相牵制；因此用户时间
// 保留可调默认值，timeout 路径优先使用真实硬件尺度。
pub const DEFAULT_CLOCK_FREQ: usize = 20_000_000;
pub const TIMEOUT_CLOCK_FREQ: usize = 100_000_000;
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
