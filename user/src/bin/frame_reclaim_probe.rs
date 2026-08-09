#![no_std]
#![no_main]
#![cfg_attr(target_arch = "loongarch64", allow(dead_code, unused_imports))]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{mmap_raw, yield_};

const PAGE_SIZE: usize = 4096;
const PRESSURE_SIZE: usize = 64 * 1024 * 1024;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_PRIVATE_ANONYMOUS: usize = 0x2 | 0x20;
const CLONE_THREAD_FLAGS: usize = (1 << 8) | (1 << 11) | (1 << 16);
const THREADS: usize = 7;
const THREAD_STACK_SIZE: usize = 16 * 1024;

#[repr(align(16))]
struct ThreadStacks([[u8; THREAD_STACK_SIZE]; THREADS]);

static EXITING: AtomicUsize = AtomicUsize::new(0);
static mut THREAD_STACKS: ThreadStacks = ThreadStacks([[0; THREAD_STACK_SIZE]; THREADS]);

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global frame_reclaim_clone_thread
    .type frame_reclaim_clone_thread, @function
frame_reclaim_clone_thread:
    addi a1, a1, -16
    sd a2, 0(a1)
    sd a3, 8(a1)
    li a2, 0
    li a3, 0
    li a4, 0
    li a7, 220
    ecall
    bnez a0, 1f
    ld t0, 0(sp)
    ld a0, 8(sp)
    jalr t0
    li a7, 93
    ecall
1:
    ret
"#
);

#[cfg(target_arch = "riscv64")]
unsafe extern "C" {
    fn frame_reclaim_clone_thread(
        flags: usize,
        stack_top: usize,
        entry: extern "C" fn(usize) -> i32,
        arg: usize,
    ) -> isize;
}

extern "C" fn exiting_thread(bit: usize) -> i32 {
    // Publish immediately before the exit syscall.  The leader deliberately
    // exits at the same time, exercising TCB deferral versus address-space
    // ownership without requiring a long-running workload.
    EXITING.fetch_or(bit, Ordering::Release);
    0
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
fn main() -> i32 {
    let base = mmap_raw(
        0,
        PRESSURE_SIZE,
        PROT_READ_WRITE,
        MAP_PRIVATE_ANONYMOUS,
        -1,
        0,
    );
    assert!(base > 0);

    // Make every page resident so a missed process-exit recycle is visible in
    // /proc/respos_health after only a few invocations.
    for offset in (0..PRESSURE_SIZE).step_by(PAGE_SIZE) {
        unsafe { ((base as usize + offset) as *mut u8).write_volatile(0xa5) };
    }

    EXITING.store(0, Ordering::Relaxed);
    for index in 0..THREADS {
        let stack_top = unsafe {
            core::ptr::addr_of_mut!(THREAD_STACKS.0[index]) as *mut u8 as usize + THREAD_STACK_SIZE
        };
        let tid = unsafe {
            frame_reclaim_clone_thread(
                CLONE_THREAD_FLAGS,
                stack_top,
                exiting_thread,
                1usize << index,
            )
        };
        assert!(tid > 0);
    }

    let all_exiting = (1usize << THREADS) - 1;
    while EXITING.load(Ordering::Acquire) != all_exiting {
        let _ = yield_();
    }
    println!(
        "FRAME_RECLAIM_PROBE_EXIT resident_mb={} threads={}",
        PRESSURE_SIZE / 1024 / 1024,
        THREADS
    );
    0
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "loongarch64")]
fn main() -> i32 {
    println!("frame_reclaim_probe skipped: RV64 diagnostic only");
    0
}
