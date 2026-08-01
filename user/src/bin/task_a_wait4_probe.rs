#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null_mut, without_provenance_mut};
use user_lib::{RUsage, exit, fork, getrusage_raw, time_get, wait4_raw};

const ECHILD: isize = 10;
const EFAULT: isize = 14;
const RUSAGE_CHILDREN: isize = -1;

fn timeval_us(tv: user_lib::TimeVal) -> usize {
    tv.sec.saturating_mul(1_000_000).saturating_add(tv.usec)
}

fn children_usage() -> RUsage {
    let mut usage = RUsage::default();
    assert_eq!(getrusage_raw(RUSAGE_CHILDREN, &mut usage), 0);
    usage
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
    assert!(child_utime > 0);
    assert!(child_stime > 0);
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

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_bad_status_retry();
    test_bad_rusage_retry();
    println!("[task-a-wait4] ALL PASS");
    0
}
