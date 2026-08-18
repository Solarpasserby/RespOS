// os/src/arch/loongarch64/config/mod.rs

mod board;
mod driver;
mod fs;
mod mm;
mod syscall;

pub use board::*;
pub use driver::*;
pub use fs::*;
pub use mm::*;
pub use syscall::*;
