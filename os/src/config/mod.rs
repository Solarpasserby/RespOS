// os/src/config.rs

//! ### 内核主要配置模块
mod board;
mod driver;
mod mm;
mod fs;
mod syscall;

pub use board::*;
pub use driver::*;
pub use mm::*;
pub use fs::*;
pub use syscall::*;
