#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null_mut;
use user_lib::{TimeSpec, clock_getres_raw, clock_gettime_raw, clock_settime_raw};

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

fn gettime(clock_id: usize) -> TimeSpec {
    let mut value = TimeSpec::default();
    assert_eq!(clock_gettime_raw(clock_id, &mut value), 0);
    value
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
        CLOCK_MONOTONIC_RAW,
        CLOCK_BOOTTIME,
    ] {
        assert_resolution(clock_id, 1_000);
    }
    for clock_id in [CLOCK_REALTIME_COARSE, CLOCK_MONOTONIC_COARSE] {
        assert_resolution(clock_id, 1_000_000);
    }
    for clock_id in [
        CLOCK_PROCESS_CPUTIME_ID,
        CLOCK_THREAD_CPUTIME_ID,
        CLOCK_REALTIME_ALARM,
        CLOCK_BOOTTIME_ALARM,
        CLOCK_TAI,
    ] {
        assert_eq!(clock_getres_raw(clock_id, null_mut()), -EINVAL);
    }
    println!("[task-a-clock] resolution/boundary PASS");
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
    test_realtime_does_not_jump_monotonic();
    println!("[task-a-clock] ALL PASS");
    0
}
