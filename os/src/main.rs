// os/src/main.rs

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
// TODO: 实现内核内部锁机制后立刻移除
#![feature(sync_unsafe_cell)]
#![feature(c_variadic)]

extern crate alloc;

// #[macro_use]
extern crate bitflags;

#[macro_use]
mod console;
mod lang_item;

pub mod arch;
// Stub symbols for lwext4 C library when musl-gcc is unavailable
#[cfg(target_arch = "riscv64")]
mod lwext4_stubs;
use arch::{config, sbi, timer, trap};

pub mod drivers;
pub mod fs;
pub mod loader;
pub mod mm;
pub mod mutex;
pub mod syscall;
pub mod task;
pub mod utils;

use core::arch::global_asm;

global_asm!(include_str!("link_app.S"));

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();

    error!("hello world");
    warn!("hello world");
    info!("hello world");
    debug!("hello world");
    trace!("hello world");

    mm::init();
    task::add_initproc();
    trap::init();
    trap::enable_timer_interrupt();
    timer::set_next_ti_trigger();

    loader::list_apps();
    task::run_tasks();

    panic!("unreachable!");
}

/// RISC-V: 使用 Rust 迭代器清零 BSS
#[cfg(target_arch = "riscv64")]
fn clear_bss() {
    unsafe extern "C" {
        unsafe fn sbss();
        unsafe fn ebss();
    }

    (sbss as *const () as usize..ebss as *const () as usize)
        .for_each(|a| unsafe { (a as *mut u8).write_volatile(0) });
}

/// LoongArch64: 使用内联汇编清零 BSS（Rust 循环在该目标上有 bug）
#[cfg(target_arch = "loongarch64")]
fn clear_bss() {
    unsafe extern "C" {
        unsafe fn sbss();
        unsafe fn ebss();
    }

    let mut start = sbss as *const () as usize;
    let end = ebss as *const () as usize;
    unsafe {
        core::arch::asm!(
            "1:",
            "bgeu   {start}, {end}, 2f",
            "st.b   $zero, {start}, 0",
            "addi.d {start}, {start}, 1",
            "b      1b",
            "2:",
            start = inout(reg) start,
            end = in(reg) end,
            options(nostack),
        );
    }
}
