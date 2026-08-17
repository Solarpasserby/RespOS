use crate::arch::config::{KERNEL_BASE, PCI_ECAM_BASE, PCI_MMIO_BASE, PCI_MMIO_SIZE};
use crate::drivers::VirtIoHalImpl;
use crate::drivers::{DevError, DevResult};
use virtio_drivers::transport::{
    DeviceType,
    pci::{
        PciTransport,
        bus::{BarInfo, Cam, Command, MemoryBarType, MmioCam, PciRoot},
        virtio_device_type,
    },
};

struct PciMemory32Allocator {
    start: u32,
    end: u32,
}

impl PciMemory32Allocator {
    const fn new(start: usize, size: usize) -> Self {
        Self {
            start: start as u32,
            end: (start + size) as u32,
        }
    }

    fn allocate(&mut self, size: u32) -> Option<u32> {
        if !size.is_power_of_two() {
            return None;
        }
        let address = self.start.checked_add(size - 1)? & !(size - 1);
        if address.checked_add(size)? > self.end {
            return None;
        }
        self.start = address + size;
        Some(address)
    }
}

pub fn find_virtio_blk_transport(index: usize) -> DevResult<PciTransport> {
    find_virtio_transport(DeviceType::Block, index)
}

pub fn find_virtio_net_transport() -> DevResult<PciTransport> {
    find_virtio_transport(DeviceType::Network, 0)
}

/// Enumerate the PCI bus once and return the transport for the `index`-th
/// virtio device of `device_type`.
///
/// BARs are assigned to *every* virtio device in bus order — not just the
/// requested type — so that block and net discovery both derive the same
/// deterministic, non-overlapping BAR layout regardless of which is run first.
#[allow(unreachable_code)] // board_ls2k1000 下早退，后续 PCI 枚举不参与编译可达性
fn find_virtio_transport(device_type: DeviceType, index: usize) -> DevResult<PciTransport> {
    // 2K1000LA 真机没有 virtio-PCI 设备，且 QEMU virt 的 PCI ECAM 基址 0x20000000
    // 在真机上未映射；直接返回无设备，避免读未映射 ECAM 触发内核页错误。
    #[cfg(feature = "board_ls2k1000")]
    {
        let _ = (device_type, index);
        return Err(DevError::BadState);
    }

    let ecam = (PCI_ECAM_BASE + KERNEL_BASE) as *mut u8;
    let cam = unsafe { MmioCam::new(ecam, Cam::Ecam) };
    let mut root = PciRoot::new(cam);

    let mut allocator = PciMemory32Allocator::new(PCI_MMIO_BASE, PCI_MMIO_SIZE);
    let mut matched = 0;
    for (device_function, info) in root.enumerate_bus(0) {
        let Some(dev_type) = virtio_device_type(&info) else {
            continue;
        };
        // Assign every preceding virtio device as well so repeated indexed
        // discovery produces the same non-overlapping BAR layout.
        for (bar_index, info) in root
            .bars(device_function)
            .map_err(|_| DevError::BadState)?
            .into_iter()
            .enumerate()
        {
            let Some(info) = info else { continue };
            match info {
                BarInfo::Memory {
                    address_type, size, ..
                } if size > 0 => {
                    let address = allocator.allocate(size as u32).ok_or(DevError::NoMemory)?;
                    match address_type {
                        MemoryBarType::Width32 => {
                            root.set_bar_32(device_function, bar_index as u8, address)
                        }
                        MemoryBarType::Width64 => {
                            root.set_bar_64(device_function, bar_index as u8, address as u64)
                        }
                        MemoryBarType::Below1MiB => return Err(DevError::Unsupported),
                    }
                }
                BarInfo::IO { .. } | BarInfo::Memory { .. } => {}
            }
        }
        root.set_command(
            device_function,
            Command::MEMORY_SPACE | Command::BUS_MASTER | Command::INTERRUPT_DISABLE,
        );
        if dev_type == device_type {
            if matched == index {
                return PciTransport::new::<VirtIoHalImpl, _>(&mut root, device_function)
                    .map_err(|_| DevError::BadState);
            }
            matched += 1;
        }
    }
    Err(DevError::BadState)
}
