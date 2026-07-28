#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null, null_mut, without_provenance_mut};
use user_lib::{
    ITimerSpec, RLimit, TimeSpec, exit, fork, getpid, prlimit64_raw, timer_create_raw,
    timer_delete_raw, timer_gettime_raw, timer_settime_raw, waitpid,
};

const CLOCK_REALTIME: usize = 0;
const RLIMIT_NOFILE: usize = 7;
const EFAULT: isize = 14;
const EINVAL: isize = 22;

fn test_timer_create_publish_after_copyout() {
    let mut first = -1;
    assert_eq!(timer_create_raw(CLOCK_REALTIME, &mut first), 0);
    assert_eq!(timer_delete_raw(first as usize), 0);

    let bad_id = without_provenance_mut::<i32>(usize::MAX);
    assert_eq!(timer_create_raw(CLOCK_REALTIME, bad_id), -EFAULT);

    let mut next = -1;
    assert_eq!(timer_create_raw(CLOCK_REALTIME, &mut next), 0);
    assert_eq!(next, first + 2);
    assert_eq!(timer_delete_raw((first + 1) as usize), -EINVAL);
    assert_eq!(timer_delete_raw(next as usize), 0);
    println!("[task-a-atomic] timer_create publish-after-copyout PASS");
}

fn test_timer_settime_copyout_atomicity() {
    let mut timerid = -1;
    assert_eq!(timer_create_raw(CLOCK_REALTIME, &mut timerid), 0);

    let new_value = ITimerSpec {
        interval: TimeSpec { sec: 1, nsec: 0 },
        value: TimeSpec { sec: 5, nsec: 0 },
    };
    let bad_old = without_provenance_mut::<ITimerSpec>(usize::MAX);
    assert_eq!(
        timer_settime_raw(timerid as usize, 0, &new_value, bad_old),
        -EFAULT
    );

    let mut current = ITimerSpec::default();
    assert_eq!(timer_gettime_raw(timerid as usize, &mut current), 0);
    assert_eq!(current, ITimerSpec::default());

    let mut old = ITimerSpec::default();
    assert_eq!(
        timer_settime_raw(timerid as usize, 0, &new_value, &mut old),
        0
    );
    assert_eq!(old, ITimerSpec::default());
    assert_eq!(timer_gettime_raw(timerid as usize, &mut current), 0);
    assert_eq!(current.interval, new_value.interval);
    assert!(current.value.sec > 0 && current.value.sec <= new_value.value.sec);
    assert_eq!(timer_delete_raw(timerid as usize), 0);
    println!("[task-a-atomic] timer_settime copyout atomicity PASS");
}

fn test_prlimit_copyout_atomicity() {
    let mut original = RLimit::default();
    assert_eq!(prlimit64_raw(0, RLIMIT_NOFILE, null(), &mut original), 0);
    assert!(original.cur > 0);
    let changed = RLimit {
        cur: original.cur - 1,
        max: original.max,
    };
    let bad_old = without_provenance_mut::<RLimit>(usize::MAX);
    assert_eq!(prlimit64_raw(0, RLIMIT_NOFILE, &changed, bad_old), -EFAULT);

    let mut after_failure = RLimit::default();
    assert_eq!(
        prlimit64_raw(0, RLIMIT_NOFILE, null(), &mut after_failure),
        0
    );
    assert_eq!(after_failure, original);

    let mut returned_old = RLimit::default();
    assert_eq!(
        prlimit64_raw(0, RLIMIT_NOFILE, &changed, &mut returned_old),
        0
    );
    assert_eq!(returned_old, original);
    let mut after_success = RLimit::default();
    assert_eq!(
        prlimit64_raw(0, RLIMIT_NOFILE, null(), &mut after_success),
        0
    );
    assert_eq!(after_success, changed);
    assert_eq!(prlimit64_raw(0, RLIMIT_NOFILE, &original, null_mut()), 0);
    println!("[task-a-atomic] prlimit copyout atomicity PASS");
}

fn test_timer_owner_exit_cleanup() {
    let owner = fork();
    assert!(owner >= 0);
    if owner == 0 {
        let owner_pid = getpid();
        for _ in 0..3 {
            let mut timerid = -1;
            assert_eq!(timer_create_raw(CLOCK_REALTIME, &mut timerid), 0);
            let armed = ITimerSpec {
                interval: TimeSpec::default(),
                value: TimeSpec { sec: 10, nsec: 0 },
            };
            assert_eq!(
                timer_settime_raw(timerid as usize, 0, &armed, null_mut()),
                0
            );
        }
        println!(
            "[task-a-atomic] timer owner {} exiting with 3 timers",
            owner_pid
        );
        exit(0);
        unreachable!();
    }
    let mut status = 0;
    assert_eq!(waitpid(owner as usize, &mut status), owner);
    assert_eq!(status, 0);

    let successor = fork();
    assert!(successor >= 0);
    if successor == 0 {
        exit(0);
        unreachable!();
    }
    assert!(successor > owner);
    assert_eq!(waitpid(successor as usize, &mut status), successor);
    assert_eq!(status, 0);
    println!(
        "[task-a-atomic] timer owner exit cleanup requested; successor pid {} PASS",
        successor
    );
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_timer_create_publish_after_copyout();
    test_timer_settime_copyout_atomicity();
    test_prlimit_copyout_atomicity();
    test_timer_owner_exit_cleanup();
    println!("[task-a-atomic] ALL PASS");
    0
}
