// os/src/arch/rv64/entry/mod.rs

mod boot;

pub use boot::enter_main;

#[cfg(feature = "board_jh7110")]
core::arch::global_asm!(include_str!("entry_jh7110.asm"));
#[cfg(not(feature = "board_jh7110"))]
core::arch::global_asm!(include_str!("entry.asm"));
