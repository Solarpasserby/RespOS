// 时钟频率，与硬件设备相关
pub const CLOCK_FREQ: usize = 12500000;
pub const MEMORY_START: usize = 0x8020_0000;
pub const MEMORY_END: usize = 0x8800_0000;

pub const VIRTIO_MMIO: &[(usize, usize)] = &[
    (0x1000_1000, 0x00_1000), // virtio-mmio-bus.0
    (0x1000_2000, 0x00_1000), // virtio-mmio-bus.1
];
