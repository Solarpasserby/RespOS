// os/src/driver.rs

mod device;
mod disk;
mod virtio;

pub use device::*;
pub use disk::Disk;
pub use virtio::VirtIoHalImpl;
#[cfg(feature = "perf_counters")]
pub(crate) use virtio::reset_bounce_perf;
pub(crate) use virtio::snapshot_bounce_perf;
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

#[cfg(target_arch = "riscv64")]
pub type NetDeviceImpl = VirtIoNetDev<VirtIoHalImpl, MmioTransport<'static>>;
#[cfg(target_arch = "loongarch64")]
pub type NetDeviceImpl = VirtIoNetDev<VirtIoHalImpl, PciTransport>;

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

impl NetDeviceImpl {
    #[cfg(target_arch = "riscv64")]
    pub fn new_device() -> DevResult<Self> {
        let transport = find_virtio_net_mmio()?;
        VirtIoNetDev::new(transport)
    }

    #[cfg(target_arch = "loongarch64")]
    pub fn new_device() -> DevResult<Self> {
        let transport = crate::arch::pci::find_virtio_net_transport()?;
        VirtIoNetDev::new(transport)
    }
}

/// Discover the first virtio-mmio network device on QEMU's RISC-V `virt`
/// machine.
///
/// QEMU places virtio-mmio devices at `0x1000_1000 + slot * 0x1000`. We read
/// the magic and device-id registers directly instead of constructing an
/// `MmioTransport` for every slot: constructing and dropping a transport over a
/// block slot would reset that device before the filesystem initializes it.
#[cfg(target_arch = "riscv64")]
fn find_virtio_net_mmio() -> DevResult<MmioTransport<'static>> {
    const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
    const VIRTIO_MMIO_STRIDE: usize = 0x1000;
    const VIRTIO_MMIO_SLOTS: usize = 8;
    const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976;
    const DEVICE_ID_NETWORK: u32 = 1;

    for slot in 0..VIRTIO_MMIO_SLOTS {
        let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
        // SAFETY: the virtio-mmio window is mapped contiguously in the kernel
        // direct map; offsets 0 and 8 are the magic and device-id registers of
        // `VirtIOHeader` respectively.
        let magic = unsafe { core::ptr::read_volatile((base + KERNEL_BASE) as *const u32) };
        let device_id = unsafe { core::ptr::read_volatile((base + KERNEL_BASE + 8) as *const u32) };
        if magic != VIRTIO_MMIO_MAGIC || device_id != DEVICE_ID_NETWORK {
            continue;
        }
        let header = NonNull::new((base + KERNEL_BASE) as *mut VirtIOHeader).unwrap();
        return unsafe {
            MmioTransport::new(header, VIRTIO_MMIO_STRIDE).map_err(|_| DevError::BadState)
        };
    }
    Err(DevError::BadState)
}
