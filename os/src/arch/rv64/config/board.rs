// RISC-V QEMU virt 机器时钟频率。
//
// 目前三类时间使用同一硬件尺度；保留拆分命名是为了和 LoongArch 的
// bench-facing wall clock / timeout / accounting 设计保持一致。
pub const HARDWARE_CLOCK_FREQ: usize = 10_000_000;
pub const USER_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const ACCOUNTING_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const MEMORY_START: usize = 0x8020_0000;
pub const MEMORY_END: usize = 0x8800_0000;

pub const VIRTIO_MMIO: &[(usize, usize)] = &[
    (0x1000_1000, 0x00_1000), // virtio-mmio-bus.0
    (0x1000_2000, 0x00_1000), // virtio-mmio-bus.1
];
