// os/src/fs/mod.rs

#[cfg(target_arch = "riscv64")]
pub mod ext4;
mod fdtable;
mod kstat;
mod mount;
mod namei;
mod page_cache;
mod path;
mod pipe;
mod stdio;
pub mod vfs;

pub use fdtable::*;
pub use kstat::*;
pub use namei::*;
pub use path::*;
pub use pipe::*;
use stdio::*;
