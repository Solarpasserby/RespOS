// os/src/arch/rv64/mod.rs

pub mod config;
mod entry;
pub mod interrupt;
pub mod mm;
pub mod sbi;
pub mod task;
pub mod timer;
pub mod trap;

pub use entry::enter_main;

use core::arch::asm;
use riscv::register::satp;

#[inline]
pub fn read_mmu_token() -> usize {
    satp::read().bits()
}
#[inline]
pub fn write_mmu_token(token: usize) {
    satp::write(token);
}

#[inline]
pub fn sfence() {
    unsafe {
        asm!("sfence.vma", options(nostack));
    }
}
