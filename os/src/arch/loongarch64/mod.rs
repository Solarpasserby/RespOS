// os/src/arch/loongarch64/mod.rs
// LoongArch 64 架构模块

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

const CSR_PGDH: usize = 0x1B;

#[inline]
pub fn read_mmu_token() -> usize {
    let token: usize;
    unsafe {
        asm!("csrrd {}, {}", out(reg) token, const CSR_PGDH);
    }
    token
}

#[inline]
pub fn write_mmu_token(token: usize) {
    unsafe {
        asm!("csrwr {}, {}", in(reg) token, const CSR_PGDH);
    }
}

#[inline]
pub fn sfence() {
    unsafe {
        asm!("invtlb 0, $zero, $zero", options(nostack));
    }
}
