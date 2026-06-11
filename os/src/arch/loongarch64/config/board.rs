// LoongArch QEMU virt 机器板级配置

// LoongArch QEMU virt 机器默认时钟频率。CPUCFG 可读到 100MHz，但当前
// QEMU/测试环境下用它做 wall-clock 换算会让 libctest 的超时等待明显变长。
pub const DEFAULT_CLOCK_FREQ: usize = 12_500_000;
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
