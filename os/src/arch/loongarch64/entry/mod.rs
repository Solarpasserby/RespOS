// os/src/arch/loongarch64/entry/mod.rs

mod boot;

pub use boot::enter_main;

core::arch::global_asm!(include_str!("entry.asm"));
