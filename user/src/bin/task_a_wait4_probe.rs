#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null_mut, without_provenance_mut};
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    RUsage, SIGUSR1, SignalAction, Tms, exit, fork, getpid, getrusage_raw, kill, sigaction,
    time_get, times, wait4_raw, yield_,
};

const ECHILD: isize = 10;
const EINTR: isize = 4;
const EFAULT: isize = 14;
const RUSAGE_CHILDREN: isize = -1;
const RUSAGE_SELF: isize = 0;
const SA_RESTART: u32 = 0x1000_0000;
static SIGNAL_COUNT: AtomicUsize = AtomicUsize::new(0);

fn signal_handler() {
    SIGNAL_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn delay_ms(ms: isize) {
    let deadline = time_get().saturating_add(ms);
    while time_get() < deadline {
        let _ = yield_();
    }
}

fn timeval_us(tv: user_lib::TimeVal) -> usize {
    tv.sec.saturating_mul(1_000_000).saturating_add(tv.usec)
}

fn children_usage() -> RUsage {
    let mut usage = RUsage::default();
    assert_eq!(getrusage_raw(RUSAGE_CHILDREN, &mut usage), 0);
    usage
}

fn self_usage() -> RUsage {
    let mut usage = RUsage::default();
    assert_eq!(getrusage_raw(RUSAGE_SELF, &mut usage), 0);
    usage
}

fn test_user_system_accounting() {
    let before = self_usage();
    let start = time_get();
    let mut value = 1usize;
    while time_get().saturating_sub(start) < 250 {
        for _ in 0..200_000 {
            value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
        core::hint::black_box(value);
    }
    let after_user = self_usage();
    let user_delta = timeval_us(after_user.ru_utime) - timeval_us(before.ru_utime);
    let first_system_delta = timeval_us(after_user.ru_stime) - timeval_us(before.ru_stime);
    assert!(user_delta > 0);
    assert_ne!(user_delta, first_system_delta);

    for _ in 0..100_000 {
        core::hint::black_box(getpid());
    }
    let after_system = self_usage();
    let system_delta = timeval_us(after_system.ru_stime) - timeval_us(after_user.ru_stime);
    assert!(system_delta > 0);

    let mut tms = Tms::default();
    assert!(times(&mut tms) >= 0);
    let final_usage = self_usage();
    let final_user_ticks = timeval_us(final_usage.ru_utime) / 10_000;
    let final_system_ticks = timeval_us(final_usage.ru_stime) / 10_000;
    assert!(tms.tms_utime.abs_diff(final_user_ticks) <= 1);
    assert!(tms.tms_stime.abs_diff(final_system_ticks) <= 1);
    println!(
        "[task-a-wait4] user/system accounting PASS user_us={} first_system_us={} syscall_system_us={}",
        user_delta, first_system_delta, system_delta
    );
}

fn spawn_child(exit_code: i32, run_ms: isize) -> isize {
    let pid = fork();
    assert!(pid >= 0);
    if pid == 0 {
        let start = time_get();
        while time_get().saturating_sub(start) < run_ms {
            core::hint::spin_loop();
        }
        exit(exit_code);
        unreachable!();
    }
    pid
}

fn test_bad_status_retry() {
    let pid = spawn_child(7, 0);
    let bad_status = without_provenance_mut::<i32>(usize::MAX);

    assert_eq!(wait4_raw(pid, bad_status, 0, null_mut()), -EFAULT);

    let mut status = 0;
    assert_eq!(wait4_raw(pid, &mut status, 0, null_mut()), pid);
    assert_eq!(status, 7 << 8);
    assert_eq!(wait4_raw(pid, &mut status, 0, null_mut()), -ECHILD);
    println!("[task-a-wait4] bad status retry PASS");
}

fn test_bad_rusage_retry() {
    let before = children_usage();
    let pid = spawn_child(9, 30);
    let mut status = 0;
    let bad_usage = without_provenance_mut::<RUsage>(usize::MAX);

    assert_eq!(wait4_raw(pid, &mut status, 0, bad_usage), -EFAULT);
    let after_failure = children_usage();
    assert_eq!(
        timeval_us(after_failure.ru_utime),
        timeval_us(before.ru_utime)
    );
    assert_eq!(
        timeval_us(after_failure.ru_stime),
        timeval_us(before.ru_stime)
    );

    let mut child_usage = RUsage::default();
    assert_eq!(wait4_raw(pid, &mut status, 0, &mut child_usage), pid);
    assert_eq!(status, 9 << 8);

    let after_success = children_usage();
    let child_utime = timeval_us(child_usage.ru_utime);
    let child_stime = timeval_us(child_usage.ru_stime);
    assert!(child_utime.saturating_add(child_stime) > 0);
    assert_eq!(
        timeval_us(after_success.ru_utime) - timeval_us(before.ru_utime),
        child_utime
    );
    assert_eq!(
        timeval_us(after_success.ru_stime) - timeval_us(before.ru_stime),
        child_stime
    );
    assert_eq!(wait4_raw(pid, &mut status, 0, null_mut()), -ECHILD);
    println!("[task-a-wait4] bad rusage retry/accounting PASS");
}

fn spawn_signaling_child(parent: usize, exit_code: i32) -> isize {
    let pid = fork();
    assert!(pid >= 0);
    if pid == 0 {
        delay_ms(30);
        assert_eq!(kill(parent, SIGUSR1), 0);
        delay_ms(60);
        exit(exit_code);
        unreachable!();
    }
    pid
}

fn test_sa_restart() {
    let parent = getpid();
    assert!(parent > 0);

    let action = SignalAction {
        handler: signal_handler as usize,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&action), None), 0);
    let child = spawn_signaling_child(parent as usize, 11);
    let mut status = 0;
    assert_eq!(wait4_raw(child, &mut status, 0, null_mut()), -EINTR);
    assert_eq!(wait4_raw(child, &mut status, 0, null_mut()), child);
    assert_eq!(status, 11 << 8);
    assert_eq!(SIGNAL_COUNT.load(Ordering::SeqCst), 1);

    let restart_action = SignalAction {
        handler: signal_handler as usize,
        flags: SA_RESTART,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&restart_action), None), 0);
    let child = spawn_signaling_child(parent as usize, 12);
    assert_eq!(wait4_raw(child, &mut status, 0, null_mut()), child);
    assert_eq!(status, 12 << 8);
    assert_eq!(SIGNAL_COUNT.load(Ordering::SeqCst), 2);
    println!("[task-a-wait4] SA_RESTART/EINTR PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_user_system_accounting();
    test_bad_status_retry();
    test_bad_rusage_retry();
    test_sa_restart();
    println!("[task-a-wait4] ALL PASS");
    0
}
