// os/src/driver.rs

mod device;
#[cfg(target_arch = "riscv64")]
mod disk;
mod virtio;

use core::ptr::NonNull;

use crate::arch::config::{KERNEL_BASE, VIRTIO_MMIO};
use device::*;
#[cfg(target_arch = "riscv64")]
pub use disk::Disk;
use virtio::*;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};

// 先支持单一的块设备，默认使用评测测试点所在的 virtio-mmio-bus.0。
pub type BlockDeviceImpl = VirtIoBlkDev<VirtIoHalImpl, MmioTransport<'static>>;

impl BlockDeviceImpl {
    pub fn new_device() -> Self {
        let (virtio0, virtio0_size) = VIRTIO_MMIO[0];
        let header = NonNull::new((virtio0 + KERNEL_BASE) as *mut VirtIOHeader).unwrap();
        let transport = unsafe {
            MmioTransport::new(header, virtio0_size)
                .expect("[kernel] VirtIO MMIO transport create failed")
        };
        VirtIoBlkDev::new(transport)
    }
}
