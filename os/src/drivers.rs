// os/src/driver.rs

mod device;
mod disk;
mod virtio;

use core::ptr::NonNull;

use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use crate::config::{KERNEL_BASE, MMIO};
use device::*;
use virtio::*;
pub use disk::Disk;

// 先支持单一的块设备
pub type BlockDeviceImpl = VirtIoBlkDev<VirtIoHalImpl, MmioTransport<'static>>;

impl BlockDeviceImpl {
    pub fn new_device() -> Self {
        let (virtio0, virtio0_size) = MMIO[1];
        let header = NonNull::new((virtio0 + KERNEL_BASE) as *mut VirtIOHeader).unwrap();
        let transport = unsafe {
            MmioTransport::new(header, virtio0_size)
                .expect("[kernel] VirtIO MMIO transport create failed")
        };
        VirtIoBlkDev::new(transport)
    }
}
