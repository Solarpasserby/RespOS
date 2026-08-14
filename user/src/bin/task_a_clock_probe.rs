#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null_mut;
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    TimeSpec, clock_getres_raw, clock_gettime_raw, clock_settime_raw, timer_create_raw,
    timer_delete_raw, yield_,
};

const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;
const CLOCK_PROCESS_CPUTIME_ID: usize = 2;
const CLOCK_THREAD_CPUTIME_ID: usize = 3;
const CLOCK_MONOTONIC_RAW: usize = 4;
const CLOCK_REALTIME_COARSE: usize = 5;
const CLOCK_MONOTONIC_COARSE: usize = 6;
const CLOCK_BOOTTIME: usize = 7;
const CLOCK_REALTIME_ALARM: usize = 8;
const CLOCK_BOOTTIME_ALARM: usize = 9;
const CLOCK_TAI: usize = 11;
const EINVAL: isize = 22;
const CLONE_THREAD_FLAGS: usize = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 16);
const THREAD_STACK_SIZE: usize = 16 * 1024;

#[repr(align(16))]
struct ThreadStack([u8; THREAD_STACK_SIZE]);

static mut THREAD_STACK: ThreadStack = ThreadStack([0; THREAD_STACK_SIZE]);
static WORKER_DONE: AtomicUsize = AtomicUsize::new(0);
static WORKER_CPU_DELTA_US: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global task_a_clock_clone_thread
    .type task_a_clock_clone_thread, @function
task_a_clock_clone_thread:
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

#[cfg(target_arch = "loongarch64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global task_a_clock_clone_thread
    .type task_a_clock_clone_thread, @function
task_a_clock_clone_thread:
    addi.d $a1, $a1, -16
    st.d $a2, $a1, 0
    st.d $a3, $a1, 8
    ori $a2, $zero, 0
    ori $a3, $zero, 0
    ori $a4, $zero, 0
    addi.w $a7, $zero, 220
    syscall 0
    bnez $a0, 1f
    ld.d $t0, $sp, 0
    ld.d $a0, $sp, 8
    jirl $ra, $t0, 0
    addi.w $a7, $zero, 93
    syscall 0
1:
    jirl $zero, $ra, 0
"#
);

unsafe extern "C" {
    fn task_a_clock_clone_thread(
        flags: usize,
        stack_top: usize,
        entry: extern "C" fn(usize) -> i32,
        arg: usize,
    ) -> isize;
}

fn gettime(clock_id: usize) -> TimeSpec {
    let mut value = TimeSpec::default();
    assert_eq!(clock_gettime_raw(clock_id, &mut value), 0);
    value
}

fn burn_cpu(seed: usize) {
    let mut checksum = seed;
    for value in 0usize..1_000_000 {
        checksum = core::hint::black_box(checksum.rotate_left(5) ^ value.wrapping_mul(17));
    }
    core::hint::black_box(checksum);
}

extern "C" fn cpu_clock_worker(_: usize) -> i32 {
    let before = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    burn_cpu(3);
    let after = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    WORKER_CPU_DELTA_US.store(after - before, Ordering::Release);
    WORKER_DONE.store(1, Ordering::Release);
    0
}

fn assert_resolution(clock_id: usize, expected_nsec: usize) {
    let mut resolution = TimeSpec::default();
    assert_eq!(clock_getres_raw(clock_id, &mut resolution), 0);
    assert_eq!(
        resolution,
        TimeSpec {
            sec: 0,
            nsec: expected_nsec,
        }
    );
}

fn to_us(value: TimeSpec) -> usize {
    value
        .sec
        .saturating_mul(1_000_000)
        .saturating_add(value.nsec / 1000)
}

fn test_resolution_and_support_boundary() {
    for clock_id in [
        CLOCK_REALTIME,
        CLOCK_MONOTONIC,
        CLOCK_PROCESS_CPUTIME_ID,
        CLOCK_THREAD_CPUTIME_ID,
        CLOCK_MONOTONIC_RAW,
        CLOCK_BOOTTIME,
    ] {
        assert_resolution(clock_id, 1_000);
    }
    for clock_id in [CLOCK_REALTIME_COARSE, CLOCK_MONOTONIC_COARSE] {
        assert_resolution(clock_id, 1_000_000);
    }
    for clock_id in [CLOCK_REALTIME_ALARM, CLOCK_BOOTTIME_ALARM, CLOCK_TAI] {
        assert_eq!(clock_getres_raw(clock_id, null_mut()), -EINVAL);
    }
    println!("[task-a-clock] resolution/boundary PASS");
}

fn test_cpu_clocks() {
    let thread_before = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    let process_before = to_us(gettime(CLOCK_PROCESS_CPUTIME_ID));

    burn_cpu(1);

    let thread_after = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    let process_after = to_us(gettime(CLOCK_PROCESS_CPUTIME_ID));
    assert!(thread_after > thread_before);
    assert!(process_after > process_before);
    assert!(process_after >= thread_after);

    for clock_id in [CLOCK_PROCESS_CPUTIME_ID, CLOCK_THREAD_CPUTIME_ID] {
        let mut timer_id = -1;
        assert_eq!(timer_create_raw(clock_id, &mut timer_id), 0);
        assert!(timer_id >= 0);
        assert_eq!(timer_delete_raw(timer_id as usize), 0);
    }
    println!("[task-a-clock] process/thread CPU clocks PASS");
}

fn test_process_clock_aggregates_threads() {
    WORKER_DONE.store(0, Ordering::Relaxed);
    WORKER_CPU_DELTA_US.store(0, Ordering::Relaxed);

    let process_before = to_us(gettime(CLOCK_PROCESS_CPUTIME_ID));
    let main_before = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    let stack_top =
        unsafe { core::ptr::addr_of_mut!(THREAD_STACK.0) as *mut u8 as usize + THREAD_STACK_SIZE };
    let tid =
        unsafe { task_a_clock_clone_thread(CLONE_THREAD_FLAGS, stack_top, cpu_clock_worker, 0) };
    assert!(tid > 0);

    while WORKER_DONE.load(Ordering::Acquire) == 0 {
        burn_cpu(5);
        let _ = yield_();
    }
    let main_after = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    let process_after = to_us(gettime(CLOCK_PROCESS_CPUTIME_ID));
    let main_delta = main_after - main_before;
    let worker_delta = WORKER_CPU_DELTA_US.load(Ordering::Acquire);
    let process_delta = process_after - process_before;
    assert!(main_delta > 0);
    assert!(worker_delta > 0);
    assert!(process_delta >= main_delta.saturating_add(worker_delta));
    println!("[task-a-clock] process aggregation PASS");
}

fn test_realtime_does_not_jump_monotonic() {
    let monotonic_before = to_us(gettime(CLOCK_MONOTONIC));
    let realtime_before = gettime(CLOCK_REALTIME);
    let shifted = TimeSpec {
        sec: realtime_before.sec.saturating_add(3_600),
        nsec: realtime_before.nsec,
    };
    assert_eq!(clock_settime_raw(CLOCK_REALTIME, &shifted), 0);

    let realtime_after = to_us(gettime(CLOCK_REALTIME));
    let monotonic_after = to_us(gettime(CLOCK_MONOTONIC));
    assert!(monotonic_after >= monotonic_before);
    assert!(monotonic_after - monotonic_before < 500_000);

    let realtime_before_us = to_us(realtime_before);
    let realtime_jump = realtime_after.saturating_sub(realtime_before_us);
    assert!(realtime_jump >= 3_599_000_000);
    assert!(realtime_jump <= 3_601_000_000);

    assert_eq!(
        clock_settime_raw(CLOCK_MONOTONIC, &TimeSpec::default()),
        -EINVAL
    );
    println!("[task-a-clock] realtime/monotonic independence PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_resolution_and_support_boundary();
    test_cpu_clocks();
    test_process_clock_aggregates_threads();
    test_realtime_does_not_jump_monotonic();
    println!("[task-a-clock] ALL PASS");
    0
}
