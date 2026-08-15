#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDWR, O_TRUNC, Stat, TimeSpec, clock_gettime_raw, close, open, stat, unlink,
    utimens,
};

const PATH: &str = "/tmp/respos_utimens_special.tmp\0";
const MISSING_PATH: &str = "/tmp/respos_utimens_special_missing.tmp\0";
const CLOCK_REALTIME: usize = 0;
const UTIME_NOW: usize = 1_073_741_823;
const UTIME_OMIT: usize = 1_073_741_822;
const EINVAL: isize = 22;

fn path_stat(path: &str) -> Stat {
    let mut value = Stat::default();
    assert_eq!(stat(path, &mut value), 0, "stat failed: {}", path);
    value
}

fn same_times(left: &Stat, right: &Stat) -> bool {
    left.st_atime == right.st_atime
        && left.st_mtime == right.st_mtime
        && left.st_ctime == right.st_ctime
}

fn realtime() -> TimeSpec {
    let mut value = TimeSpec::default();
    assert_eq!(clock_gettime_raw(CLOCK_REALTIME, &mut value), 0);
    value
}

fn milliseconds(value: TimeSpec) -> usize {
    value
        .sec
        .saturating_mul(1000)
        .saturating_add(value.nsec / 1_000_000)
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(PATH);
    let _ = unlink(MISSING_PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0, "create failed: {}", fd);
    assert_eq!(close(fd as usize), 0);

    let mut times = [
        TimeSpec { sec: 100, nsec: 0 },
        TimeSpec { sec: 200, nsec: 0 },
    ];
    assert_eq!(utimens(PATH, &times), 0);
    let baseline = path_stat(PATH);
    assert_eq!(baseline.st_atime.sec, 100);
    assert_eq!(baseline.st_mtime.sec, 200);

    times = [
        TimeSpec {
            sec: usize::MAX,
            nsec: UTIME_OMIT,
        },
        TimeSpec {
            sec: usize::MAX,
            nsec: UTIME_OMIT,
        },
    ];
    assert_eq!(utimens(PATH, &times), 0);
    let omitted = path_stat(PATH);
    assert!(same_times(&omitted, &baseline));
    assert_eq!(utimens(MISSING_PATH, &times), 0);

    times = [
        TimeSpec {
            sec: usize::MAX,
            nsec: UTIME_OMIT,
        },
        TimeSpec { sec: 300, nsec: 0 },
    ];
    assert_eq!(utimens(PATH, &times), 0);
    let one_omitted = path_stat(PATH);
    assert_eq!(one_omitted.st_atime, baseline.st_atime);
    assert_eq!(one_omitted.st_mtime.sec, 300);
    assert_eq!(one_omitted.st_mtime.nsec, 0);

    let before_now = realtime();
    times = [
        TimeSpec {
            sec: usize::MAX,
            nsec: UTIME_NOW,
        },
        TimeSpec {
            sec: usize::MAX,
            nsec: UTIME_OMIT,
        },
    ];
    assert_eq!(utimens(PATH, &times), 0);
    let after_now = realtime();
    let now_and_omit = path_stat(PATH);
    assert!(milliseconds(now_and_omit.st_atime) >= milliseconds(before_now).saturating_sub(1000));
    assert!(milliseconds(now_and_omit.st_atime) <= milliseconds(after_now));
    assert_eq!(now_and_omit.st_mtime, one_omitted.st_mtime);

    let before_invalid = path_stat(PATH);
    times = [
        TimeSpec {
            sec: 400,
            nsec: 1_000_000_000,
        },
        TimeSpec {
            sec: usize::MAX,
            nsec: UTIME_OMIT,
        },
    ];
    assert_eq!(utimens(PATH, &times), -EINVAL);
    assert!(same_times(&path_stat(PATH), &before_invalid));

    times = [
        TimeSpec {
            sec: usize::MAX,
            nsec: UTIME_NOW,
        },
        TimeSpec {
            sec: 500,
            nsec: usize::MAX,
        },
    ];
    assert_eq!(utimens(PATH, &times), -EINVAL);
    assert!(same_times(&path_stat(PATH), &before_invalid));

    assert_eq!(unlink(PATH), 0);
    println!("UTIMENS_SPECIAL PASS omit=pass now=pass invalid_nsec=pass missing_double_omit=pass");
    0
}
