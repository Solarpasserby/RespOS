use crate::drivers::{DevError, DevResult, VirtIoBlkDev, VirtIoHalImpl};
use core::panic::PanicInfo;
use core::ptr::NonNull;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};

core::arch::global_asm!(include_str!("../arch/rv64/entry/entry.asm"));

pub const ROOT_DISK_BASE_BLOCK: usize = 0;
pub const ALLOW_MISSING_ROOT: bool = false;
pub const HARDWARE_CLOCK_FREQ: usize = 10_000_000;
pub const USER_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const ACCOUNTING_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const MEMORY_START: usize = 0x8020_0000;
pub const MEMORY_END: usize = 0x9000_0000;
pub const MAX_PHYSICAL_MEMORY_END: usize = 0x4_8000_0000;
pub const RAM_BASE: usize = 0x8000_0000;
pub const MMIO: &[(usize, usize)] = &[
    (0x0010_1000, 0x00_1000),
    (0x1000_0000, 0x00_1000),
    (0x1000_1000, 0x00_8000),
];
const VIRTIO_BLOCK_MMIO: &[(usize, usize)] = &[(0x1000_1000, 0x00_1000), (0x1000_2000, 0x00_1000)];

pub type BlockDevice = VirtIoBlkDev<VirtIoHalImpl, MmioTransport<'static>>;

pub fn direct_console_putchar(c: u8) {
    const UART_BASE: usize = 0x1000_0000;
    const LSR_TX_EMPTY: u8 = 1 << 5;
    let base = crate::config::KERNEL_BASE + UART_BASE;
    unsafe {
        while (core::ptr::read_volatile((base + 5) as *const u8) & LSR_TX_EMPTY) == 0 {}
        core::ptr::write_volatile(base as *mut u8, c);
    }
}

pub fn early_init(_hart_id: usize, fdt_pa: usize) {
    crate::config::init_physical_memory_end(fdt_pa);
}

pub fn init_kernel() {
    crate::trap::init();
    crate::mm::init();
    crate::arch::sbi::mark_direct_uart_ready();
    crate::banner::print_boot_banner();
    crate::syscall::init_realtime_from_rtc();
    crate::net::init();
}

pub fn start_secondary_cpus() {
    let boot_hart = crate::arch::smp::boot_hart();
    crate::arch::smp::publish_boot_ready(boot_hart);
    crate::arch::smp::start_secondary_harts(boot_hart);
}

pub fn release_secondary_cpus() {}

pub fn new_block_device(index: usize) -> DevResult<BlockDevice> {
    let &(base, size) = VIRTIO_BLOCK_MMIO.get(index).ok_or(DevError::InvalidParam)?;
    let header = NonNull::new((base + crate::config::KERNEL_BASE) as *mut VirtIOHeader).unwrap();
    let transport = unsafe { MmioTransport::new(header, size).map_err(|_| DevError::BadState)? };
    VirtIoBlkDev::new(transport)
}

pub fn report_panic(info: &PanicInfo) {
    super::report_default_panic(info);
}

pub fn shutdown(failure: bool) -> ! {
    crate::arch::sbi::shutdown(failure)
}

pub fn poweroff() -> ! {
    shutdown(false)
}
