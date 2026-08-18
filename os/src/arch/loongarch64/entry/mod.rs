// os/src/arch/loongarch64/entry/mod.rs

mod boot;

pub use boot::enter_main;

#[cfg(feature = "board_ls2k1000")]
core::arch::global_asm!(include_str!("entry_ls2k1000.asm"));
#[cfg(not(feature = "board_ls2k1000"))]
core::arch::global_asm!(include_str!("entry.asm"));
