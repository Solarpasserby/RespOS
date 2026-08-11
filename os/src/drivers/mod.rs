// os/src/driver.rs

mod device;
mod disk;
mod virtio;

pub use device::*;
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

#[cfg(target_arch = "riscv64")]
pub type BlockDeviceImpl = VirtIoBlkDev<VirtIoHalImpl, MmioTransport<'static>>;
#[cfg(target_arch = "loongarch64")]
pub type BlockDeviceImpl = VirtIoBlkDev<VirtIoHalImpl, PciTransport>;

impl BlockDeviceImpl {
    #[cfg(target_arch = "riscv64")]
    pub fn new_device(index: usize) -> DevResult<Self> {
        let &(virtio0, virtio0_size) = VIRTIO_MMIO.get(index).ok_or(DevError::InvalidParam)?;
        let header = NonNull::new((virtio0 + KERNEL_BASE) as *mut VirtIOHeader).unwrap();
        let transport =
            unsafe { MmioTransport::new(header, virtio0_size).map_err(|_| DevError::BadState)? };
        VirtIoBlkDev::new(transport)
    }

    #[cfg(target_arch = "loongarch64")]
    pub fn new_device(index: usize) -> DevResult<Self> {
        let transport = crate::arch::pci::find_virtio_blk_transport(index)?;
        VirtIoBlkDev::new(transport)
    }
}
