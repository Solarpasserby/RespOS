// LoongArch QEMU virt 机器板级配置

// LoongArch QEMU virt 机器默认时钟频率 (与 RV64 一致)
pub const CLOCK_FREQ: usize = 12500000;
// QEMU loongarch64 virt with `-m 128M` maps RAM in low memory.
pub const MEMORY_START: usize = 0;
pub const MEMORY_END: usize = 0x0800_0000;

pub const PCI_ECAM_BASE: usize = 0x2000_0000;
pub const PCI_ECAM_SIZE: usize = 0x1000_0000;
pub const PCI_MMIO_BASE: usize = 0x4000_0000;
pub const PCI_MMIO_SIZE: usize = 0x1000_0000;

// MMIO 设备地址区间 (QEMU loongarch64 virt 平台)
pub const MMIO: &[(usize, usize)] = &[
    (0x1000_1000, 0x00_1000),       // Virtio Block
    (0x1fe0_0000, 0x00_1000),       // UART
    (0x0010_0000, 0x00_2000),       // VIRT_TEST/RTC
    (PCI_ECAM_BASE, PCI_ECAM_SIZE), // PCIe ECAM
    (PCI_MMIO_BASE, PCI_MMIO_SIZE), // PCI BAR memory window
];
