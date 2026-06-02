use crate::arch::config::{KERNEL_BASE, PCI_ECAM_BASE, PCI_MMIO_BASE, PCI_MMIO_SIZE};
use crate::drivers::VirtIoHalImpl;
use virtio_drivers::transport::{
    DeviceType,
    pci::{
        PciTransport,
        bus::{BarInfo, Cam, Command, DeviceFunction, MemoryBarType, MmioCam, PciRoot},
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

    fn allocate(&mut self, size: u32) -> u32 {
        assert!(size.is_power_of_two());
        let address = (self.start + size - 1) & !(size - 1);
        assert!(
            address + size <= self.end,
            "[kernel] PCI BAR memory exhausted"
        );
        self.start = address + size;
        address
    }
}

fn allocate_bars(root: &mut PciRoot<MmioCam<'static>>, device_function: DeviceFunction) {
    let mut allocator = PciMemory32Allocator::new(PCI_MMIO_BASE, PCI_MMIO_SIZE);
    for (bar_index, info) in root
        .bars(device_function)
        .expect("[kernel] PCI BAR probing failed")
        .into_iter()
        .enumerate()
    {
        let Some(info) = info else { continue };
        match info {
            BarInfo::Memory {
                address_type, size, ..
            } if size > 0 => match address_type {
                MemoryBarType::Width32 => {
                    root.set_bar_32(
                        device_function,
                        bar_index as u8,
                        allocator.allocate(size as u32),
                    );
                }
                MemoryBarType::Width64 => {
                    root.set_bar_64(
                        device_function,
                        bar_index as u8,
                        allocator.allocate(size as u32) as u64,
                    );
                }
                MemoryBarType::Below1MiB => {
                    panic!("[kernel] unsupported PCI BAR below 1MiB");
                }
            },
            BarInfo::IO { .. } | BarInfo::Memory { .. } => {}
        }
    }
    root.set_command(
        device_function,
        Command::MEMORY_SPACE | Command::BUS_MASTER | Command::INTERRUPT_DISABLE,
    );
}

pub fn find_virtio_blk_transport() -> PciTransport {
    let ecam = (PCI_ECAM_BASE + KERNEL_BASE) as *mut u8;
    let cam = unsafe { MmioCam::new(ecam, Cam::Ecam) };
    let mut root = PciRoot::new(cam);

    for (device_function, info) in root.enumerate_bus(0) {
        if virtio_device_type(&info) != Some(DeviceType::Block) {
            continue;
        }
        allocate_bars(&mut root, device_function);
        return PciTransport::new::<VirtIoHalImpl, _>(&mut root, device_function)
            .expect("[kernel] VirtIO PCI transport create failed");
    }

    panic!("[kernel] virtio-blk PCI device not found");
}
