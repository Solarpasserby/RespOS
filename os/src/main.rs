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

global_asm!(include_str!("entry.asm"));
global_asm!(include_str!("link_app.S"));

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();
    trap::init();
    loader::load_app();
    
    trap::enable_timer_interrupt();

    mm::init();
    
    panic!("unreachable!");

    // timer::set_next_ti_trigger();
    // task::start_running_tasks();
    // panic!("unreachable!");
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