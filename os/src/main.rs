#![no_std]
#![no_main]

use core::arch::global_asm;

#[macro_use]
mod console;
mod lang_item;
mod sbi;
mod sync;
pub mod config;
pub mod task;
pub mod loader;
pub mod syscall;
pub mod trap;

global_asm!(include_str!("entry.asm"));
global_asm!(include_str!("link_app.S"));

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();
    println!("Hello, world!");
    trap::init();
    loader::load_app();
    task::start_running_tasks();
    // panic!("unreachable!");
}

fn clear_bss() {
    unsafe extern "C" {
        safe fn sbss();
        safe fn ebss();
    }

    (sbss as *const() as usize..ebss as *const() as usize).for_each(|a| {
        unsafe { (a as *mut u8).write_volatile(0) }
    });
}