#[path = "../drivers/jh7110_sd.rs"]
mod sd;

use crate::drivers::{DevError, DevResult};
use core::panic::PanicInfo;
use sd::SdCard;

core::arch::global_asm!(include_str!("../arch/rv64/entry/entry_jh7110.asm"));

pub const ROOT_DISK_BASE_BLOCK: usize = 526336;
pub const ALLOW_MISSING_ROOT: bool = false;
pub const HARDWARE_CLOCK_FREQ: usize = 4_000_000;
pub const USER_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const ACCOUNTING_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const MEMORY_START: usize = 0x4020_0000;
pub const MEMORY_END: usize = 0x8000_0000;
pub const MAX_PHYSICAL_MEMORY_END: usize = 0x1_4000_0000;
pub const RAM_BASE: usize = 0x4000_0000;
pub const MMIO: &[(usize, usize)] = &[
    (0x1000_0000, 0x00_1000),
    (0x0200_0000, 0x00_1000),
    (0x0c00_0000, 0x400_0000),
    (0x1601_0000, 0x4_0000),
];

pub type BlockDevice = SdCard;

pub fn direct_console_putchar(c: u8) {
    const UART_BASE: usize = 0x1000_0000;
    const UART_REG_SHIFT: usize = 2;
    const UART_THR_REG: usize = 0;
    const UART_LSR_REG: usize = 5;
    const LSR_TX_EMPTY: u32 = 1 << 5;
    let base = crate::config::KERNEL_BASE + UART_BASE;
    unsafe {
        while (core::ptr::read_volatile((base + (UART_LSR_REG << UART_REG_SHIFT)) as *const u32)
            & LSR_TX_EMPTY)
            == 0
        {}
        core::ptr::write_volatile(
            (base + (UART_THR_REG << UART_REG_SHIFT)) as *mut u32,
            u32::from(c),
        );
    }
}

pub fn early_init(hart_id: usize, fdt_pa: usize) {
    println!(
        "[vf2] Hello RespOS on VisionFive 2 (hart_id={}, dtb={:#x})",
        hart_id, fdt_pa
    );
    crate::config::init_physical_memory_end(fdt_pa);
    println!(
        "[vf2] physical_memory_end = {:#x}",
        crate::config::physical_memory_end()
    );
}

pub fn init_kernel() {
    crate::trap::init();
    crate::mm::init();
    crate::arch::sbi::mark_direct_uart_ready();
    crate::banner::print_boot_banner();
}

pub fn start_secondary_cpus() {}

pub fn release_secondary_cpus() {}

pub fn new_block_device(index: usize) -> DevResult<BlockDevice> {
    if index != 0 {
        return Err(DevError::Unsupported);
    }
    SdCard::new()
}

pub fn report_panic(info: &PanicInfo) {
    super::report_default_panic(info);
}

pub fn shutdown(failure: bool) -> ! {
    crate::arch::sbi::shutdown(failure)
}

pub fn poweroff() -> ! {
    // OpenSBI's PMIC poweroff path fails on VF2 and repeatedly prints I2C
    // errors. A cold reboot provides deterministic process termination.
    crate::arch::sbi::restart()
}
