#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null_mut;
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    clock_getres_raw, clock_gettime_raw, clock_settime_raw, close, exit, fadvise64, fork, fsync,
    getrusage_raw, lseek, mmap, munmap, nanosleep, open, pipe, read, sched_getaffinity,
    sched_setaffinity, timer_create_raw, timer_delete_raw, unlink, wait4_raw, waitid_raw, write,
    yield_, RUsage, TimeSpec, O_CREATE, O_RDWR, O_TRUNC, SEEK_SET,
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
const RUSAGE_SELF: isize = 0;
const RUSAGE_CHILDREN: isize = -1;
const RUSAGE_THREAD: isize = 1;
const PAGE_SIZE: usize = 4096;
const PROT_READ_WRITE: usize = 0x3;
const MAP_PRIVATE_ANONYMOUS: usize = 0x22;
const MAP_PRIVATE: usize = 0x2;
const PROT_READ: usize = 0x1;
const POSIX_FADV_DONTNEED: usize = 4;
const RUSAGE_IO_PATH: &str = "/tmp/respos_rusage_io.tmp\0";
const RUSAGE_CHILD_IO_PATH: &str = "/tmp/respos_rusage_child_io.tmp\0";
const CLONE_THREAD_FLAGS: usize = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 16);
const THREAD_STACK_SIZE: usize = 16 * 1024;

#[repr(align(16))]
struct ThreadStack([u8; THREAD_STACK_SIZE]);

static mut THREAD_STACK: ThreadStack = ThreadStack([0; THREAD_STACK_SIZE]);
static WORKER_DONE: AtomicUsize = AtomicUsize::new(0);
static WORKER_CPU_DELTA_US: AtomicUsize = AtomicUsize::new(0);
static WORKER_RUSAGE_USER_US: AtomicUsize = AtomicUsize::new(0);
static WORKER_RUSAGE_SYSTEM_US: AtomicUsize = AtomicUsize::new(0);

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
    let usage_before = rusage(RUSAGE_THREAD);
    let before = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    let mut usage_after = usage_before;
    for round in 0..64 {
        burn_cpu(3 + round);
        usage_after = rusage(RUSAGE_THREAD);
        if usage_total_us(usage_after) > usage_total_us(usage_before) {
            break;
        }
    }
    let after = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    WORKER_CPU_DELTA_US.store(after - before, Ordering::Release);
    WORKER_RUSAGE_USER_US.store(
        timeval_us(usage_after.ru_utime) - timeval_us(usage_before.ru_utime),
        Ordering::Release,
    );
    WORKER_RUSAGE_SYSTEM_US.store(
        timeval_us(usage_after.ru_stime) - timeval_us(usage_before.ru_stime),
        Ordering::Release,
    );
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

fn timeval_us(value: user_lib::TimeVal) -> usize {
    value
        .sec
        .saturating_mul(1_000_000)
        .saturating_add(value.usec)
}

fn rusage(who: isize) -> RUsage {
    let mut usage = RUsage::default();
    assert_eq!(getrusage_raw(who, &mut usage), 0);
    usage
}

fn usage_total_us(usage: RUsage) -> usize {
    timeval_us(usage.ru_utime).saturating_add(timeval_us(usage.ru_stime))
}

fn assert_linux_zero_legacy_rusage(usage: RUsage) {
    assert_eq!(usage.ru_ixrss, 0);
    assert_eq!(usage.ru_idrss, 0);
    assert_eq!(usage.ru_isrss, 0);
    assert_eq!(usage.ru_nswap, 0);
    assert_eq!(usage.ru_msgsnd, 0);
    assert_eq!(usage.ru_msgrcv, 0);
    assert_eq!(usage.ru_nsignals, 0);
}

fn sleep_for_voluntary_switch() {
    let request = TimeSpec {
        sec: 0,
        nsec: 1_000_000,
    };
    let mut remaining = TimeSpec::default();
    assert_eq!(nanosleep(&request, &mut remaining), 0);
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

fn test_realtime_has_calendar_epoch() {
    let realtime = gettime(CLOCK_REALTIME);
    assert!(
        realtime.sec >= 1_704_067_200,
        "CLOCK_REALTIME was not initialized from a calendar clock"
    );
    assert!(realtime.nsec < 1_000_000_000);
    println!(
        "[task-a-clock] realtime calendar epoch PASS sec={}",
        realtime.sec
    );
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
    WORKER_RUSAGE_USER_US.store(0, Ordering::Relaxed);
    WORKER_RUSAGE_SYSTEM_US.store(0, Ordering::Relaxed);

    let process_before = to_us(gettime(CLOCK_PROCESS_CPUTIME_ID));
    let main_before = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    let process_usage_before = rusage(RUSAGE_SELF);
    let main_usage_before = rusage(RUSAGE_THREAD);
    let stack_top =
        unsafe { core::ptr::addr_of_mut!(THREAD_STACK.0) as *mut u8 as usize + THREAD_STACK_SIZE };
    let tid =
        unsafe { task_a_clock_clone_thread(CLONE_THREAD_FLAGS, stack_top, cpu_clock_worker, 0) };
    assert!(tid > 0);

    let mut main_usage_after = main_usage_before;
    for round in 0..64 {
        burn_cpu(5 + round);
        main_usage_after = rusage(RUSAGE_THREAD);
        if usage_total_us(main_usage_after) > usage_total_us(main_usage_before) {
            break;
        }
    }
    while WORKER_DONE.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }
    let main_after = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    let process_after = to_us(gettime(CLOCK_PROCESS_CPUTIME_ID));
    let process_usage_after = rusage(RUSAGE_SELF);
    let main_delta = main_after - main_before;
    let worker_delta = WORKER_CPU_DELTA_US.load(Ordering::Acquire);
    let process_delta = process_after - process_before;
    assert!(main_delta > 0);
    assert!(worker_delta > 0);
    assert!(process_delta >= main_delta.saturating_add(worker_delta));
    let main_usage_delta = usage_total_us(main_usage_after) - usage_total_us(main_usage_before);
    let worker_usage_delta = WORKER_RUSAGE_USER_US
        .load(Ordering::Acquire)
        .saturating_add(WORKER_RUSAGE_SYSTEM_US.load(Ordering::Acquire));
    let process_usage_delta =
        usage_total_us(process_usage_after) - usage_total_us(process_usage_before);
    assert!(main_usage_delta > 0);
    assert!(worker_usage_delta > 0);
    assert!(process_usage_delta.saturating_add(20_000) >= main_usage_delta + worker_usage_delta);
    let mut invalid = RUsage::default();
    assert_eq!(getrusage_raw(2, &mut invalid), -EINVAL);
    println!("[task-a-clock] process aggregation/RUSAGE_THREAD PASS");
}

fn touch_anonymous_pages(pages: usize) -> usize {
    let len = pages * PAGE_SIZE;
    let addr = mmap(0, len, PROT_READ_WRITE, MAP_PRIVATE_ANONYMOUS, -1, 0);
    assert!(addr > 0);
    for page in 0..pages {
        unsafe {
            (addr as *mut u8)
                .add(page * PAGE_SIZE)
                .write_volatile((page as u8).wrapping_add(1));
        }
    }
    addr as usize
}

fn create_clean_evicted_file(path: &str) -> isize {
    let fd = open(path, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0);
    let page = [0x5au8; PAGE_SIZE];
    assert_eq!(write(fd as usize, &page), PAGE_SIZE as isize);
    assert_eq!(write(fd as usize, &page), PAGE_SIZE as isize);
    assert_eq!(fsync(fd as usize), 0);
    assert_eq!(fadvise64(fd as usize, 0, 0, POSIX_FADV_DONTNEED), 0);
    fd
}

fn test_rusage_file_io_and_major_fault() {
    let fd = open(RUSAGE_IO_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0);
    let page = [0x3cu8; PAGE_SIZE];
    let before_write = rusage(RUSAGE_THREAD);
    assert_eq!(write(fd as usize, &page), PAGE_SIZE as isize);
    assert_eq!(write(fd as usize, &page), PAGE_SIZE as isize);
    let after_first_dirty = rusage(RUSAGE_THREAD);
    assert!(after_first_dirty.ru_oublock - before_write.ru_oublock >= 16);

    assert_eq!(lseek(fd as usize, 0, SEEK_SET), 0);
    assert_eq!(write(fd as usize, &page), PAGE_SIZE as isize);
    let after_repeat_dirty = rusage(RUSAGE_THREAD);
    assert_eq!(after_repeat_dirty.ru_oublock, after_first_dirty.ru_oublock);
    assert_eq!(fsync(fd as usize), 0);
    assert_eq!(fadvise64(fd as usize, 0, 0, POSIX_FADV_DONTNEED), 0);

    let mapping = mmap(0, 2 * PAGE_SIZE, PROT_READ, MAP_PRIVATE, fd, 0);
    assert!(mapping > 0);
    let before_major = rusage(RUSAGE_THREAD);
    let first = unsafe { (mapping as *const u8).read_volatile() };
    assert_eq!(first, 0x3c);
    let after_major = rusage(RUSAGE_THREAD);
    assert!(after_major.ru_majflt > before_major.ru_majflt);
    assert!(after_major.ru_inblock > before_major.ru_inblock);

    let second = unsafe { (mapping as *const u8).add(PAGE_SIZE).read_volatile() };
    assert_eq!(second, 0x3c);
    let after_readahead = rusage(RUSAGE_THREAD);
    assert_eq!(after_readahead.ru_majflt, after_major.ru_majflt);
    assert!(after_readahead.ru_minflt > after_major.ru_minflt);

    assert_eq!(munmap(mapping as usize, 2 * PAGE_SIZE), 0);
    assert_eq!(close(fd as usize), 0);
    assert_eq!(unlink(RUSAGE_IO_PATH), 0);
    println!("[task-a-clock] rusage block-io/cold-major PASS");
}

fn exercise_child_file_io() {
    let fd = create_clean_evicted_file(RUSAGE_CHILD_IO_PATH);
    assert_eq!(lseek(fd as usize, 0, SEEK_SET), 0);
    let mut byte = [0u8; 1];
    assert_eq!(read(fd as usize, &mut byte), 1);
    assert_eq!(byte[0], 0x5a);
    assert_eq!(close(fd as usize), 0);
    assert_eq!(unlink(RUSAGE_CHILD_IO_PATH), 0);
}

fn test_rusage_fault_rss_and_switch_fields() {
    let self_before = rusage(RUSAGE_SELF);
    let thread_before = rusage(RUSAGE_THREAD);
    let fault_pages = (self_before.ru_maxrss.max(0) as usize / 4).saturating_add(512);
    let addr = touch_anonymous_pages(fault_pages);
    let thread_after_fault = rusage(RUSAGE_THREAD);
    let self_after_fault = rusage(RUSAGE_SELF);
    assert!(thread_after_fault.ru_minflt - thread_before.ru_minflt >= fault_pages as isize);
    assert!(self_after_fault.ru_minflt - self_before.ru_minflt >= fault_pages as isize);
    assert!(self_after_fault.ru_maxrss > self_before.ru_maxrss);
    assert!(self_after_fault.ru_maxrss >= (fault_pages * 4) as isize);
    assert!(thread_after_fault.ru_maxrss >= self_after_fault.ru_maxrss);

    let mut original_affinity = 0usize;
    assert!(sched_getaffinity(0, &mut original_affinity) > 0);
    let one_cpu = 1usize << original_affinity.trailing_zeros();
    assert_eq!(sched_setaffinity(0, one_cpu), 0);
    let _ = yield_();

    // RespOS crosses its idle stack on every timer tick.  With no competing
    // runnable task that internal handoff must not look like a Linux task
    // context switch.
    let solo_before = rusage(RUSAGE_THREAD);
    let solo_cpu_start = to_us(gettime(CLOCK_THREAD_CPUTIME_ID));
    while to_us(gettime(CLOCK_THREAD_CPUTIME_ID)) - solo_cpu_start < 50_000 {
        burn_cpu(0x8800);
    }
    let solo_after = rusage(RUSAGE_THREAD);
    assert_eq!(solo_after.ru_nivcsw, solo_before.ru_nivcsw);

    let voluntary_before = solo_after;
    assert_eq!(yield_(), 0);
    let voluntary_after = rusage(RUSAGE_THREAD);
    assert_eq!(voluntary_after.ru_nvcsw, voluntary_before.ru_nvcsw);

    // Keep a competitor blocked until the baseline is captured, then make
    // both processes runnable on the same CPU.  The following timer switch is
    // a real prev != next transition and must remain involuntary.
    let mut gate = [-1i32; 2];
    assert_eq!(pipe(&mut gate), 0);
    let competitor = fork();
    assert!(competitor >= 0);
    if competitor == 0 {
        assert_eq!(close(gate[1] as usize), 0);
        let mut byte = [0u8; 1];
        assert_eq!(read(gate[0] as usize, &mut byte), 1);
        assert_eq!(close(gate[0] as usize), 0);
        for round in 0..64 {
            burn_cpu(0x8900 + round);
        }
        let _ = exit(0);
        loop {
            core::hint::spin_loop();
        }
    }
    assert_eq!(close(gate[0] as usize), 0);
    let involuntary_before = rusage(RUSAGE_THREAD).ru_nivcsw;
    assert_eq!(write(gate[1] as usize, &[1]), 1);
    assert_eq!(close(gate[1] as usize), 0);
    let mut involuntary_after = involuntary_before;
    for round in 0..256 {
        burn_cpu(0x9000 + round);
        involuntary_after = rusage(RUSAGE_THREAD).ru_nivcsw;
        if involuntary_after > involuntary_before {
            break;
        }
    }
    assert!(involuntary_after > involuntary_before);
    let mut competitor_status = -1;
    let mut competitor_usage = RUsage::default();
    assert_eq!(
        wait4_raw(competitor, &mut competitor_status, 0, &mut competitor_usage),
        competitor
    );
    assert_eq!(competitor_status, 0);
    assert_eq!(sched_setaffinity(0, original_affinity), 0);
    assert_eq!(munmap(addr, fault_pages * PAGE_SIZE), 0);
    let after_unmap = rusage(RUSAGE_SELF);
    assert!(after_unmap.ru_maxrss >= self_after_fault.ru_maxrss);
    println!("[task-a-clock] rusage fault/rss/context-switch PASS");
    println!("[task-a-clock] rusage actual-switch accounting PASS");
}

fn test_rusage_linux_zero_legacy_fields() {
    assert_linux_zero_legacy_rusage(rusage(RUSAGE_THREAD));
    assert_linux_zero_legacy_rusage(rusage(RUSAGE_SELF));
    assert_linux_zero_legacy_rusage(rusage(RUSAGE_CHILDREN));

    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_linux_zero_legacy_rusage(rusage(RUSAGE_THREAD));
        let _ = exit(0);
        loop {
            core::hint::spin_loop();
        }
    }
    let mut status = -1;
    let mut child_usage = RUsage::default();
    assert_eq!(wait4_raw(child, &mut status, 0, &mut child_usage), child);
    assert_eq!(status, 0);
    assert_linux_zero_legacy_rusage(child_usage);
    assert_linux_zero_legacy_rusage(rusage(RUSAGE_CHILDREN));
    println!("[task-a-clock] rusage Linux-zero legacy fields PASS");
}

fn test_rusage_reaped_child_fields() {
    const CHILD_PAGES: usize = 128;
    let children_before = rusage(RUSAGE_CHILDREN);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        let addr = touch_anonymous_pages(CHILD_PAGES);
        exercise_child_file_io();
        sleep_for_voluntary_switch();
        for round in 0..32 {
            burn_cpu(0xa000 + round);
            if rusage(RUSAGE_SELF).ru_nivcsw != 0 {
                break;
            }
        }
        assert_eq!(munmap(addr, CHILD_PAGES * PAGE_SIZE), 0);
        let _ = exit(0);
        loop {
            core::hint::spin_loop();
        }
    }

    let mut status = -1;
    let mut child_usage = RUsage::default();
    assert_eq!(wait4_raw(child, &mut status, 0, &mut child_usage), child);
    assert_eq!(status, 0);
    let children_after = rusage(RUSAGE_CHILDREN);
    assert!(child_usage.ru_minflt >= CHILD_PAGES as isize);
    assert!(child_usage.ru_maxrss >= (CHILD_PAGES * 4) as isize);
    assert!(child_usage.ru_nvcsw >= 1);
    assert!(child_usage.ru_inblock > 0);
    assert!(child_usage.ru_oublock >= 16);
    assert_eq!(
        children_after.ru_minflt - children_before.ru_minflt,
        child_usage.ru_minflt
    );
    assert_eq!(
        children_after.ru_majflt - children_before.ru_majflt,
        child_usage.ru_majflt
    );
    assert_eq!(
        children_after.ru_nvcsw - children_before.ru_nvcsw,
        child_usage.ru_nvcsw
    );
    assert_eq!(
        children_after.ru_nivcsw - children_before.ru_nivcsw,
        child_usage.ru_nivcsw
    );
    assert_eq!(
        children_after.ru_inblock - children_before.ru_inblock,
        child_usage.ru_inblock
    );
    assert_eq!(
        children_after.ru_oublock - children_before.ru_oublock,
        child_usage.ru_oublock
    );
    assert_eq!(
        children_after.ru_maxrss,
        children_before.ru_maxrss.max(child_usage.ru_maxrss)
    );
    println!("[task-a-clock] rusage wait4/children fields PASS");
}

fn test_rusage_waitid_raw_fields() {
    const CHILD_PAGES: usize = 32;
    const P_PID: usize = 1;
    const WEXITED: usize = 4;
    let children_before = rusage(RUSAGE_CHILDREN);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        let _addr = touch_anonymous_pages(CHILD_PAGES);
        sleep_for_voluntary_switch();
        let _ = exit(0);
        loop {
            core::hint::spin_loop();
        }
    }

    let mut siginfo = [0u8; 128];
    let mut child_usage = RUsage::default();
    assert_eq!(
        waitid_raw(
            P_PID,
            child as usize,
            siginfo.as_mut_ptr(),
            WEXITED,
            &mut child_usage,
        ),
        0
    );
    let children_after = rusage(RUSAGE_CHILDREN);
    assert!(child_usage.ru_minflt >= CHILD_PAGES as isize);
    assert!(child_usage.ru_nvcsw >= 1);
    assert_eq!(
        children_after.ru_minflt - children_before.ru_minflt,
        child_usage.ru_minflt
    );
    assert_eq!(
        children_after.ru_nvcsw - children_before.ru_nvcsw,
        child_usage.ru_nvcsw
    );
    println!("[task-a-clock] raw waitid rusage/children fields PASS");
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
    test_realtime_has_calendar_epoch();
    test_cpu_clocks();
    test_process_clock_aggregates_threads();
    test_rusage_fault_rss_and_switch_fields();
    test_rusage_linux_zero_legacy_fields();
    test_rusage_file_io_and_major_fault();
    test_rusage_reaped_child_fields();
    test_rusage_waitid_raw_fields();
    test_realtime_does_not_jump_monotonic();
    println!("[task-a-clock] ALL PASS");
    0
}
