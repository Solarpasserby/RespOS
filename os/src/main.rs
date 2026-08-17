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
    config::init_physical_memory_end(opaque);
    rust_main_high()
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(no_mangle)]
pub fn rust_main(argc: usize, argv: usize) -> ! {
    clear_bss();
    config::init_physical_memory_end(argc, argv);

    // Stage 2：接通 MMU。真机经 U-Boot `go` 进入时 CRMD.PG 已为 1（DMW 窗口），
    // enable_boot_paging 会建立最小高半区过渡页表并安装内核根页表，随后跳入高半区。
    // 跳转前用 uncached UART 打一行诊断，便于定位 enable_boot_paging / 跳转前的故障。
    #[cfg(feature = "board_ls2k1000")]
    {
        sbi::early_print("RespOS 2K1000LA: enabling MMU, mem_end=");
        sbi::early_print_hex(config::physical_memory_end());
        sbi::early_print("\n");
    }

    arch::enable_boot_paging();
    unsafe {
        #[cfg(feature = "board_ls2k1000")]
        arch::jump_to_high_half(rust_main_high_ls2k1000 as usize);
        #[cfg(not(feature = "board_ls2k1000"))]
        arch::jump_to_high_half(rust_main_high as usize);
    }
}

/// 2K1000LA Stage 2 高半区入口：验证 40-bit VA 下 3 级页表 + frame allocator + direct map。
/// 暂不接 interrupt/timer/FS/net/SMP（Stage 3+），打印里程碑后停在这里，方便逐层排障。
#[cfg(all(target_arch = "loongarch64", feature = "board_ls2k1000"))]
fn rust_main_high_ls2k1000() -> ! {
    arch::enable_kernel_extensions();

    // 跑到这里说明高半区取指/数据访问都通过了 3 级页表翻译（40-bit VA 的关键验证点）。
    sbi::early_print("[RespOS 2K1000LA] entered high half (3-level paging OK)\n");

    mm::init();
    // mm::init 会初始化 heap + frame allocator + direct map 并 activate 内核页表。
    sbi::early_print("[RespOS 2K1000LA] mm::init OK, free frames=");
    sbi::early_print_hex(mm::free_frame_count());
    sbi::early_print("\n");

    // heap 已就绪，验证 alloc/println! 路径（走 uncached UART）。
    println!("[RespOS 2K1000LA] heap OK");
    arch::idle()
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

#[cfg(any(target_arch = "riscv64", not(feature = "board_ls2k1000")))]
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
