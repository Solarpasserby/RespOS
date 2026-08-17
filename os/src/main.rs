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
    #[cfg(feature = "board_jh7110")]
    {
        println!(
            "[vf2] Hello RespOS on VisionFive 2 (hart_id={}, dtb={:#x})",
            hart_id, opaque
        );
        // Stage 2：FDT 内存发现 + 正式 MMU（frame allocator + heap + direct map）。
        // 之后暂驻留；用户态/块设备/网络/SMP 属于 Stage 3+。
        config::init_physical_memory_end(opaque);
        println!(
            "[vf2] physical_memory_end = {:#x}",
            config::physical_memory_end()
        );
        trap::init();
        mm::init();
        println!(
            "[vf2] mm::init ok, free_frames = {}",
            mm::free_frame_count()
        );
        // Stage 5：SD 卡驱动自检（初始化 + 读块 0，MBR 结尾应为 55 AA）。
        match crate::drivers::jh7110_sd::SdCard::new() {
            Ok(card) => {
                println!("[vf2] SD card init ok");
                let mut blk = [0u8; 512];
                match crate::drivers::BlockDevice::read_block(&card, 0, &mut blk) {
                    Ok(()) => {
                        println!(
                            "[vf2] read block 0 ok, tail={:02x}{:02x}",
                            blk[510], blk[511]
                        );
                    }
                    Err(e) => println!("[vf2] read block 0 failed: {:?}", e),
                }
            }
            Err(e) => println!("[vf2] SD card init failed: {:?}", e),
        }
        // Stage 3：timer 中断 + trap。每 1 秒打印一次，验证 SBI set_timer → mtimecmp
        // → supervisor timer interrupt → trap 重装 → WFI 唤醒 全链路。若 timer 不工作，
        // 首次 WFI 会永久挂住、无任何 tick 输出。
        trap::enable_timer_interrupt();
        timer::set_next_ti_trigger();
        let mut last_ms = timer::get_time_ms();
        loop {
            arch::wait_for_interrupt();
            let now_ms = timer::get_time_ms();
            if now_ms.saturating_sub(last_ms) >= 1000 {
                println!("[vf2] timer tick at {} ms", now_ms);
                last_ms = now_ms;
            }
        }
    }
    #[cfg(not(feature = "board_jh7110"))]
    {
        config::init_physical_memory_end(opaque);
        rust_main_high()
    }
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();
    config::init_physical_memory_end();

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

// Stage 1 的 `board_jh7110` 构建在 `rust_main` 里打印 Hello 后驻留，暂不进入本函数。
#[cfg_attr(feature = "board_jh7110", allow(dead_code))]
fn rust_main_high() -> ! {
    #[cfg(target_arch = "loongarch64")]
    arch::enable_kernel_extensions();

    #[cfg(target_arch = "loongarch64")]
    timer::init_clock_freq();

    trap::init();
    mm::init();
    syscall::init_realtime_from_rtc();
    net::init();
    task::add_initproc();
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    {
        task::init_per_cpu_idle_tasks();
    }
    #[cfg(target_arch = "riscv64")]
    {
        let boot_hart = arch::smp::boot_hart();
        arch::smp::publish_boot_ready(boot_hart);
        arch::smp::start_secondary_harts(boot_hart);
    }
    #[cfg(target_arch = "loongarch64")]
    {
        arch::smp::publish_boot_ready();
        arch::smp::start_secondary_harts();
    }
    trap::enable_timer_interrupt();
    timer::set_next_ti_trigger();
    #[cfg(target_arch = "loongarch64")]
    arch::smp::release_secondary_harts();

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
