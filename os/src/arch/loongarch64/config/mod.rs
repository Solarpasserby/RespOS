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

// 兼容 RV64 命名：共享代码使用 VIRTIO_MMIO
pub use board::MMIO as VIRTIO_MMIO;
