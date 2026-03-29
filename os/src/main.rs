// os/src/main.rs

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

// #[macro_use]
extern crate bitflags;

#[macro_use]
mod console;
mod lang_item;
mod sbi;
mod sync;
pub mod config;
pub mod task;
pub mod loader;
pub mod syscall;
pub mod timer;
pub mod trap;
pub mod mm;

use core::arch::global_asm;

use crate::loader::list_apps;

global_asm!(include_str!("entry.asm"));
global_asm!(include_str!("link_app.S"));

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss(); // 手动清理 .bss
    mm::init();
    task::add_initproc();
    trap::init();
    trap::enable_timer_interrupt();
    timer::set_next_ti_trigger();

    loader::list_apps();
    task::run_tasks();

    list_apps();

    panic!("unreachable!");
}

fn clear_bss() {
    unsafe extern "C" {
        unsafe fn sbss();
        unsafe fn ebss();
    }

    (sbss as *const() as usize..ebss as *const() as usize).for_each(|a| {
        unsafe { (a as *mut u8).write_volatile(0) }
    });
}