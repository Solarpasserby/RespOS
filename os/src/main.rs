// os/src/main.rs

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
// TODO: 实现内核内部锁机制后立刻移除
#![feature(sync_unsafe_cell)]

extern crate alloc;

// #[macro_use]
extern crate bitflags;

#[macro_use]
mod console;
mod lang_item;
mod sbi;
pub mod config;
pub mod drivers;
pub mod task;
pub mod loader;
pub mod syscall;
pub mod timer;
pub mod trap;
pub mod mm;
pub mod fs;
pub mod utils;

use core::arch::global_asm;


global_asm!(include_str!("entry.asm"));
global_asm!(include_str!("link_app.S"));

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();

    // ⭐ 加在这里
    info!("=== KERNEL START ===");
    error!("this is error");
    warn!("this is warn");
    info!("this is info");
    debug!("this is debug (may not show)");
    trace!("this is trace (may not show)");

    mm::init();
    task::add_initproc();
    trap::init();
    trap::enable_timer_interrupt();
    timer::set_next_ti_trigger();

    loader::list_apps();
    task::run_tasks();

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