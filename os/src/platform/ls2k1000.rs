#[path = "../drivers/ahci.rs"]
mod ahci;
#[path = "../arch/loongarch64/fdt.rs"]
pub(crate) mod fdt;
#[path = "../arch/loongarch64/liointc.rs"]
mod liointc;

use crate::drivers::{DevError, DevResult};
use ahci::AhciBlockDevice;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

core::arch::global_asm!(include_str!("../arch/loongarch64/entry/entry_ls2k1000.asm"));

pub const ROOT_DISK_BASE_BLOCK: usize = 0;
pub const ALLOW_MISSING_ROOT: bool = true;
pub const FIRMWARE_PAGING_ACTIVE: bool = true;
pub const PRESERVE_UNCACHED_DMW0: bool = true;
pub const UNCACHED_MMIO: bool = true;
pub const HAS_VIRTIO_PCI: bool = false;
pub const MEMORY_START: usize = 0x0020_0000;
pub const LOW_MEMORY_END: usize = 0x0b00_0000;
pub const HIGH_MEMORY_START: usize = 0x9000_0000;
pub const MEMORY_END: usize = 0xc000_0000;
pub const MAX_PHYSICAL_MEMORY_END: usize = 0xc000_0000;
pub const UART_BASE: usize = 0x1fe2_0000;
pub const MMIO: &[(usize, usize)] = &[
    (0x1fe2_0000, 0x1000),
    (0x1fe0_1400, 0x1000),
    (0x4004_0000, 0x1000),
    (0x4005_0000, 0x1000),
    (0x400e_0000, 0x1000),
];

pub type BlockDevice = AhciBlockDevice;

pub fn early_init(argc: usize, argv: usize) {
    crate::config::init_physical_memory_end(argc, argv);
    crate::arch::sbi::early_print("RespOS 2K1000LA: enabling MMU, mem_end=");
    crate::arch::sbi::early_print_hex(crate::config::physical_memory_end());
    crate::arch::sbi::early_print("\n");
}

pub fn discover_physical_memory_end(argc: usize, argv: usize) -> Option<usize> {
    let fdt_addr = fdt::fdt_addr_from_boot_args(argc, argv)?;
    let end = unsafe { fdt::memory_end_from_fdt(fdt_addr) }?;
    (end > MEMORY_START && end <= (1usize << 48)).then_some(end)
}

pub fn init_kernel() {
    crate::arch::enable_kernel_extensions();
    crate::arch::sbi::early_print("[RespOS 2K1000LA] entered high half (3-level paging OK)\n");

    crate::mm::init();
    crate::arch::sbi::early_print("[RespOS 2K1000LA] mm::init OK, free frames=");
    crate::arch::sbi::early_print_hex(crate::mm::free_frame_count());
    crate::arch::sbi::early_print("\n");
    println!("[RespOS 2K1000LA] heap OK");

    liointc::init();
    crate::arch::sbi::early_print("[RespOS 2K1000LA] LIOINTC masked\n");
    crate::trap::init();
}

pub fn start_secondary_cpus() {}

pub fn release_secondary_cpus() {}

pub fn new_block_device(index: usize) -> DevResult<BlockDevice> {
    if index != 0 {
        return Err(DevError::InvalidParam);
    }
    AhciBlockDevice::new()
}

pub fn report_panic(info: &PanicInfo) {
    static PANIC_PRINTED: AtomicBool = AtomicBool::new(false);
    if !PANIC_PRINTED.swap(true, Ordering::Relaxed) {
        crate::arch::sbi::early_print("Panicked at ");
        if let Some(location) = info.location() {
            crate::arch::sbi::early_print(location.file());
            crate::arch::sbi::early_print(":");
            crate::arch::sbi::early_print_hex(location.line() as usize);
        }
        crate::arch::sbi::early_print("\n");
    }
}

pub fn report_trap(tag: &str, cx: &crate::arch::trap::TrapContext) {
    static TRAP_DIAG_PRINTED: AtomicBool = AtomicBool::new(false);
    if TRAP_DIAG_PRINTED.swap(true, Ordering::Relaxed) {
        return;
    }
    crate::arch::sbi::early_print("[trap] ");
    crate::arch::sbi::early_print(tag);
    crate::arch::sbi::early_print(" est=");
    crate::arch::sbi::early_print_hex(crate::arch::register::estat::read());
    crate::arch::sbi::early_print(" era=");
    crate::arch::sbi::early_print_hex(cx.era);
    crate::arch::sbi::early_print(" badv=");
    crate::arch::sbi::early_print_hex(crate::arch::register::badv::read());
    crate::arch::sbi::early_print(" sp=");
    crate::arch::sbi::early_print_hex(cx.get_sp());
    crate::arch::sbi::early_print(" inst=");
    crate::arch::sbi::early_print_hex(
        unsafe { core::ptr::read_volatile(cx.era as *const u32) } as usize
    );
    crate::arch::sbi::early_print("\n");
}

pub fn report_fault_in_failure(va: usize, sp: usize, error: isize) {
    static PRINTED: AtomicBool = AtomicBool::new(false);
    if PRINTED.swap(true, Ordering::Relaxed) {
        return;
    }
    crate::arch::sbi::early_print("[trap] faultin-fail va=");
    crate::arch::sbi::early_print_hex(va);
    crate::arch::sbi::early_print(" sp=");
    crate::arch::sbi::early_print_hex(sp);
    crate::arch::sbi::early_print(" err=");
    crate::arch::sbi::early_print_hex(error as usize);
    crate::arch::sbi::early_print("\n");
}

pub fn shutdown(failure: bool) -> ! {
    crate::arch::sbi::shutdown(failure)
}

pub fn poweroff() -> ! {
    shutdown(false)
}
