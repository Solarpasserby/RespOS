// os/src/arch/rv64/mod.rs

mod entry;
pub mod config;
pub mod sbi;
pub mod timer;
pub mod trap;
pub mod mm;
pub mod task;

pub use entry::enter_main;
