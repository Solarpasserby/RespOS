// os/src/driver.rs

mod device;
mod disk;
mod virtio;

use device::*;
pub use disk::Disk;
pub use virtio::VirtIoHalImpl;
use virtio::*;
#[cfg(target_arch = "loongarch64")]
use virtio_drivers::transport::pci::PciTransport;
#[cfg(target_arch = "riscv64")]
use {
    crate::arch::config::{KERNEL_BASE, VIRTIO_MMIO},
    core::ptr::NonNull,
    virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader},
};

// 先支持单一的块设备，默认使用评测测试点所在的 virtio-mmio-bus.0。
#[cfg(target_arch = "riscv64")]
pub type BlockDeviceImpl = VirtIoBlkDev<VirtIoHalImpl, MmioTransport<'static>>;
#[cfg(target_arch = "loongarch64")]
pub type BlockDeviceImpl = VirtIoBlkDev<VirtIoHalImpl, PciTransport>;

impl BlockDeviceImpl {
    #[cfg(target_arch = "riscv64")]
    pub fn new_device() -> Self {
        let (virtio0, virtio0_size) = VIRTIO_MMIO[0];
        let header = NonNull::new((virtio0 + KERNEL_BASE) as *mut VirtIOHeader).unwrap();
        let transport = unsafe {
            MmioTransport::new(header, virtio0_size)
                .expect("[kernel] VirtIO MMIO transport create failed")
        };
        VirtIoBlkDev::new(transport)
    }

    #[cfg(target_arch = "loongarch64")]
    pub fn new_device() -> Self {
        let transport = crate::arch::pci::find_virtio_blk_transport();
        VirtIoBlkDev::new(transport)
    }
}
