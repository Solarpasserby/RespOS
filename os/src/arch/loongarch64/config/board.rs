// LoongArch QEMU virt 机器板级配置

// LoongArch QEMU virt 机器默认时钟频率 (与 RV64 一致)
pub const CLOCK_FREQ: usize = 12500000;
// 物理内存结束地址 (128M, 与 RV64 相同)
pub const MEMORY_END: usize = 0x8800_0000;

// MMIO 设备地址区间 (QEMU loongarch64 virt 平台)
pub const MMIO: &[(usize, usize)] = &[
    (0x0010_0000, 0x00_2000), // VIRT_TEST/RTC
    (0x1000_1000, 0x00_1000), // Virtio Block
];
