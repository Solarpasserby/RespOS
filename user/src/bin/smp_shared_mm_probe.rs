#![no_std]
#![no_main]
#![cfg_attr(target_arch = "loongarch64", allow(dead_code, unused_imports))]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{mmap_raw, munmap, sched_getaffinity, sched_setaffinity, yield_};

const PAGE_SIZE: usize = 4096;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_PRIVATE_ANONYMOUS: usize = 0x2 | 0x20;
const MAP_FIXED: usize = 0x10;
const CLONE_THREAD_FLAGS: usize = (1 << 8) | (1 << 11) | (1 << 16);
const THREAD_STACK_SIZE: usize = 16 * 1024;
const ROUNDS: u32 = 100;

const PHASE_IDLE: u32 = 0;
const PHASE_READ_OLD: u32 = 1;
const PHASE_ACK_OLD: u32 = 2;
const PHASE_READ_NEW: u32 = 3;
const PHASE_ACK_NEW: u32 = 4;
const PHASE_DONE: u32 = 5;
const PHASE_EXITED: u32 = 6;

#[repr(C)]
struct Control {
    phase: AtomicU32,
    observed: AtomicU32,
}

#[repr(align(16))]
struct ThreadStack([u8; THREAD_STACK_SIZE]);

static mut THREAD_STACK: ThreadStack = ThreadStack([0; THREAD_STACK_SIZE]);

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global smp_shared_mm_clone_thread
    .type smp_shared_mm_clone_thread, @function
smp_shared_mm_clone_thread:
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
    fn smp_shared_mm_clone_thread(
        flags: usize,
        stack_top: usize,
        entry: extern "C" fn(usize) -> i32,
        arg: usize,
    ) -> isize;
}

fn wait_for(control: &Control, phase: u32) {
    while control.phase.load(Ordering::Acquire) != phase {
        core::hint::spin_loop();
    }
}

extern "C" fn reader_thread(arg: usize) -> i32 {
    let control = unsafe { &*(arg as *const Control) };
    let data_addr = arg + PAGE_SIZE;

    loop {
        let phase = control.phase.load(Ordering::Acquire);
        match phase {
            PHASE_READ_OLD => {
                let value = unsafe { (data_addr as *const u32).read_volatile() };
                control.observed.store(value, Ordering::Relaxed);
                control.phase.store(PHASE_ACK_OLD, Ordering::Release);
            }
            PHASE_READ_NEW => {
                let value = unsafe { (data_addr as *const u32).read_volatile() };
                control.observed.store(value, Ordering::Relaxed);
                control.phase.store(PHASE_ACK_NEW, Ordering::Release);
            }
            PHASE_DONE => {
                control.phase.store(PHASE_EXITED, Ordering::Release);
                return 0;
            }
            _ => core::hint::spin_loop(),
        }
    }
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
fn main() -> i32 {
    let mut online = 0usize;
    assert!(sched_getaffinity(0, &mut online) > 0);
    assert!(
        online.count_ones() >= 2,
        "shared-MM probe needs at least two CPUs"
    );
    let cpu0 = 1usize << online.trailing_zeros();
    let cpu1 = 1usize << (online & !cpu0).trailing_zeros();

    assert_eq!(sched_setaffinity(0, cpu0), 0);
    let _ = yield_();

    let base = mmap_raw(
        0,
        PAGE_SIZE * 2,
        PROT_READ_WRITE,
        MAP_PRIVATE_ANONYMOUS,
        -1,
        0,
    );
    assert!(base > 0);
    let base = base as usize;
    let control = unsafe { &*(base as *const Control) };
    control.phase.store(PHASE_IDLE, Ordering::Relaxed);
    control.observed.store(0, Ordering::Relaxed);

    let stack_top =
        unsafe { core::ptr::addr_of_mut!(THREAD_STACK.0) as *mut u8 as usize + THREAD_STACK_SIZE };
    let tid =
        unsafe { smp_shared_mm_clone_thread(CLONE_THREAD_FLAGS, stack_top, reader_thread, base) };
    assert!(tid > 0);
    assert_eq!(sched_setaffinity(tid, cpu1), 0);

    let data_addr = base + PAGE_SIZE;
    for round in 1..=ROUNDS {
        let old_value = 0x1000_0000 | round;
        unsafe { (data_addr as *mut u32).write_volatile(old_value) };
        control.phase.store(PHASE_READ_OLD, Ordering::Release);
        wait_for(control, PHASE_ACK_OLD);
        assert_eq!(control.observed.load(Ordering::Relaxed), old_value);

        control.phase.store(PHASE_IDLE, Ordering::Release);
        assert_eq!(munmap(data_addr, PAGE_SIZE), 0);
        assert_eq!(
            mmap_raw(
                data_addr,
                PAGE_SIZE,
                PROT_READ_WRITE,
                MAP_PRIVATE_ANONYMOUS | MAP_FIXED,
                -1,
                0,
            ),
            data_addr as isize
        );

        let new_value = 0x2000_0000 | round;
        unsafe { (data_addr as *mut u32).write_volatile(new_value) };
        control.phase.store(PHASE_READ_NEW, Ordering::Release);
        wait_for(control, PHASE_ACK_NEW);
        assert_eq!(control.observed.load(Ordering::Relaxed), new_value);
        control.phase.store(PHASE_IDLE, Ordering::Release);
    }

    control.phase.store(PHASE_DONE, Ordering::Release);
    wait_for(control, PHASE_EXITED);
    println!("SMP_SHARED_MM_PROBE_PASS rounds={}", ROUNDS);
    0
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "loongarch64")]
fn main() -> i32 {
    println!("smp_shared_mm_probe skipped: RV64 SMP only");
    0
}
