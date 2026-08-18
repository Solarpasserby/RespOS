//! Compile-time selected machine platform.
//!
//! Architecture modules implement ISA mechanisms. This module owns machine
//! policy: boot ordering, optional devices/services, SMP bring-up, root-disk
//! layout, panic reporting, and shutdown behavior.

#[cfg(not(all(target_arch = "loongarch64", feature = "board_ls2k1000")))]
use core::panic::PanicInfo;

#[cfg(all(feature = "board_jh7110", feature = "board_ls2k1000"))]
compile_error!("board_jh7110 and board_ls2k1000 are mutually exclusive");
#[cfg(all(feature = "board_jh7110", not(target_arch = "riscv64")))]
compile_error!("board_jh7110 requires target_arch=riscv64");
#[cfg(all(feature = "board_ls2k1000", not(target_arch = "loongarch64")))]
compile_error!("board_ls2k1000 requires target_arch=loongarch64");

#[cfg(all(target_arch = "riscv64", feature = "board_jh7110"))]
mod jh7110;
#[cfg(all(target_arch = "loongarch64", feature = "board_ls2k1000"))]
mod ls2k1000;
#[cfg(all(target_arch = "loongarch64", not(feature = "board_ls2k1000")))]
mod qemu_loongarch64;
#[cfg(all(target_arch = "riscv64", not(feature = "board_jh7110")))]
mod qemu_rv64;

#[cfg(all(target_arch = "riscv64", feature = "board_jh7110"))]
pub use jh7110::*;
#[cfg(all(target_arch = "loongarch64", feature = "board_ls2k1000"))]
pub use ls2k1000::*;
#[cfg(all(target_arch = "loongarch64", not(feature = "board_ls2k1000")))]
pub use qemu_loongarch64::*;
#[cfg(all(target_arch = "riscv64", not(feature = "board_jh7110")))]
pub use qemu_rv64::*;

#[cfg(not(all(target_arch = "loongarch64", feature = "board_ls2k1000")))]
fn report_default_panic(info: &PanicInfo) {
    if let Some(location) = info.location() {
        println!(
            "Panicked at {}:{} {}",
            location.file(),
            location.line(),
            info.message()
        );
    } else {
        println!("Panicked: {}", info.message());
    }
}
