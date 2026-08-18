use crate::drivers::{DevResult, VirtIoBlkDev, VirtIoHalImpl};
use core::panic::PanicInfo;
use virtio_drivers::transport::pci::PciTransport;

core::arch::global_asm!(include_str!("../arch/loongarch64/entry/entry.asm"));

pub const ROOT_DISK_BASE_BLOCK: usize = 0;
pub const ALLOW_MISSING_ROOT: bool = false;
pub const FIRMWARE_PAGING_ACTIVE: bool = false;
pub const PRESERVE_UNCACHED_DMW0: bool = false;
pub const UNCACHED_MMIO: bool = false;
pub const HAS_VIRTIO_PCI: bool = true;
pub const MEMORY_START: usize = 0;
pub const LOW_MEMORY_END: usize = 0x1000_0000;
pub const HIGH_MEMORY_START: usize = 0x8000_0000;
pub const MEMORY_END: usize = 0x3_7000_0000;
pub const MAX_PHYSICAL_MEMORY_END: usize = 0x9_7000_0000;
pub const UART_BASE: usize = 0x1fe0_01e0;
pub const MMIO: &[(usize, usize)] = &[
    (0x1000_1000, 0x00_1000),
    (0x100d_0000, 0x00_1000),
    (0x100e_0000, 0x00_1000),
    (0x1fe0_0000, 0x00_1000),
    (0x0010_0000, 0x00_2000),
    (0x2000_0000, 0x1000_0000),
    (0x4000_0000, 0x1000_0000),
];

pub type BlockDevice = VirtIoBlkDev<VirtIoHalImpl, PciTransport>;

pub fn early_init(argc: usize, argv: usize) {
    crate::config::init_physical_memory_end(argc, argv);
}

pub fn discover_physical_memory_end(_argc: usize, _argv: usize) -> Option<usize> {
    const FW_CFG_DATA: usize = 0x1e02_0000;
    const FW_CFG_SELECTOR: usize = FW_CFG_DATA + 8;
    const FW_CFG_RAM_SIZE: u16 = 0x0003;

    let ram_size = unsafe {
        core::ptr::write_volatile(FW_CFG_SELECTOR as *mut u16, FW_CFG_RAM_SIZE.to_be());
        u64::from_le(core::ptr::read_volatile(FW_CFG_DATA as *const u64))
    };
    if ram_size < LOW_MEMORY_END as u64 {
        return None;
    }
    let high_size = ram_size.checked_sub(LOW_MEMORY_END as u64)?;
    let end = usize::try_from((HIGH_MEMORY_START as u64).checked_add(high_size)?).ok()?;
    let minimum = HIGH_MEMORY_START + crate::config::KERNEL_HEAP_SIZE;
    Some(end.clamp(minimum, MAX_PHYSICAL_MEMORY_END))
}

pub fn init_kernel() {
    crate::arch::enable_kernel_extensions();
    crate::timer::init_clock_freq();
    crate::trap::init();
    crate::mm::init();
    crate::banner::print_boot_banner();
    crate::syscall::init_realtime_from_rtc();
    crate::net::init();
}

pub fn start_secondary_cpus() {
    crate::arch::smp::publish_boot_ready();
    crate::arch::smp::start_secondary_harts();
}

pub fn release_secondary_cpus() {
    crate::arch::smp::release_secondary_harts();
}

pub fn new_block_device(index: usize) -> DevResult<BlockDevice> {
    let transport = crate::arch::pci::find_virtio_blk_transport(index)?;
    VirtIoBlkDev::new(transport)
}

pub fn report_panic(info: &PanicInfo) {
    super::report_default_panic(info);
}

pub fn report_trap(_tag: &str, _cx: &crate::arch::trap::TrapContext) {}

pub fn report_fault_in_failure(_va: usize, _sp: usize, _error: isize) {}

pub fn shutdown(failure: bool) -> ! {
    crate::arch::sbi::shutdown(failure)
}

pub fn poweroff() -> ! {
    shutdown(false)
}
