// os/src/config.rs

//! ### 内核配置模块
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
