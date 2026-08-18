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
mod banner;
mod lang_item;
mod platform;

pub mod arch;

// Keep the architecture facade at the crate root: kernel subsystems import
// `crate::{config,sbi,timer,trap}` without depending on an ISA module path.
use arch::{config, sbi, timer, trap};

pub mod drivers;
pub mod fs;
pub mod loader;
pub mod mm;
pub mod mutex;
pub mod net;
pub mod perf;
pub mod signal;
pub mod syscall;
pub mod task;
pub mod utils;

use core::arch::global_asm;

global_asm!(include_str!("link_app.S"));

#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
pub fn rust_main(hart_id: usize, opaque: usize) -> ! {
    if !arch::smp::claim_boot_hart(hart_id) {
        arch::smp::secondary_main(hart_id, opaque);
    }
    clear_bss();
    arch::smp::init_current_hart(hart_id);
    platform::early_init(hart_id, opaque);
    rust_main_high()
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(no_mangle)]
pub fn rust_main(argc: usize, argv: usize) -> ! {
    clear_bss();
    platform::early_init(argc, argv);

    arch::enable_boot_paging();
    unsafe {
        arch::jump_to_high_half(rust_main_high as usize);
    }
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(no_mangle)]
pub fn rust_secondary_main() -> ! {
    arch::enable_secondary_boot_paging();
    unsafe {
        arch::jump_to_high_half(rust_secondary_main_high as usize);
    }
}

#[cfg(target_arch = "loongarch64")]
fn rust_secondary_main_high() -> ! {
    arch::enable_kernel_extensions();
    mm::activate_kernel_space();
    arch::disable_low_direct_map();
    trap::init();
    arch::smp::secondary_online();
    trap::enable_timer_interrupt();
    timer::set_next_ti_trigger();
    task::run_tasks();
}

/// Shared high-half lifecycle. Machine-specific ordering and optional services
/// are selected by `platform`; the scheduler handoff remains common.
fn rust_main_high() -> ! {
    platform::init_kernel();
    task::add_initproc();
    task::init_per_cpu_idle_tasks();
    platform::start_secondary_cpus();
    trap::enable_timer_interrupt();
    timer::set_next_ti_trigger();
    platform::release_secondary_cpus();

    #[cfg(feature = "verbose_boot")]
    loader::list_apps();
    task::run_tasks();
}

/// 启动早期清零 BSS。
fn clear_bss() {
    unsafe extern "C" {
        unsafe fn sbss();
        unsafe fn ebss();
    }

    #[cfg(target_arch = "loongarch64")]
    unsafe {
        let mut cur = sbss as usize;
        let end = ebss as usize;

        while cur.wrapping_add(core::mem::size_of::<usize>()) <= end {
            core::arch::asm!(
                "st.d $zero, {addr}, 0",
                addr = in(reg) cur,
                options(nostack, preserves_flags)
            );
            cur = cur.wrapping_add(core::mem::size_of::<usize>());
        }
        while cur < end {
            core::arch::asm!(
                "st.b $zero, {addr}, 0",
                addr = in(reg) cur,
                options(nostack, preserves_flags)
            );
            cur = cur.wrapping_add(1);
        }
    }

    #[cfg(target_arch = "riscv64")]
    (sbss as *const () as usize..ebss as *const () as usize)
        .for_each(|a| unsafe { (a as *mut u8).write_volatile(0) });
}
