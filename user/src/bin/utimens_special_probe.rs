#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDWR, O_TRUNC, Stat, TimeSpec, chmod, clock_gettime_raw, close, exit, fork,
    futimens, open, setgid, setuid, stat, unlink, utimens, waitpid,
};

const PATH: &str = "/tmp/respos_utimens_special.tmp\0";
const MISSING_PATH: &str = "/tmp/respos_utimens_special_missing.tmp\0";
const CLOCK_REALTIME: usize = 0;
const UTIME_NOW: usize = 1_073_741_823;
const UTIME_OMIT: usize = 1_073_741_822;
const EINVAL: isize = 22;
const EPERM: isize = 1;
const EACCES: isize = 13;

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

fn verify_nonowner_permissions(fd: usize, mode: usize, now_result: isize) {
    assert_eq!(chmod(PATH, mode), 0);
    let child = fork();
    assert!(child >= 0, "permission fork failed: {}", child);
    if child == 0 {
        assert_eq!(setgid(65534), 0);
        assert_eq!(setuid(65534), 0);

        let double_omit = [
            TimeSpec {
                sec: usize::MAX,
                nsec: UTIME_OMIT,
            },
            TimeSpec {
                sec: usize::MAX,
                nsec: UTIME_OMIT,
            },
        ];
        assert_eq!(utimens(PATH, &double_omit), 0);
        assert_eq!(futimens(fd, &double_omit), 0);

        let explicit = [
            TimeSpec { sec: 600, nsec: 0 },
            TimeSpec { sec: 700, nsec: 0 },
        ];
        assert_eq!(utimens(PATH, &explicit), -EPERM);
        assert_eq!(futimens(fd, &explicit), -EPERM);

        let now_and_omit = [
            TimeSpec {
                sec: usize::MAX,
                nsec: UTIME_NOW,
            },
            TimeSpec {
                sec: usize::MAX,
                nsec: UTIME_OMIT,
            },
        ];
        assert_eq!(utimens(PATH, &now_and_omit), -EPERM);
        assert_eq!(futimens(fd, &now_and_omit), -EPERM);

        let double_now = [
            TimeSpec {
                sec: usize::MAX,
                nsec: UTIME_NOW,
            },
            TimeSpec {
                sec: usize::MAX,
                nsec: UTIME_NOW,
            },
        ];
        assert_eq!(utimens(PATH, &double_now), now_result);
        assert_eq!(futimens(fd, &double_now), now_result);
        exit(0);
        unreachable!();
    }

    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(
        status, 0,
        "permission child mode={:o} status={}",
        mode, status
    );
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(PATH);
    let _ = unlink(MISSING_PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0, "create failed: {}", fd);
    let fd = fd as usize;

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
            nsec: 123_456_789,
        },
        TimeSpec {
            sec: 2_147_483_648,
            nsec: 987_654_321,
        },
    ];
    assert_eq!(utimens(PATH, &times), 0);
    let extended = path_stat(PATH);
    assert_eq!(extended.st_atime.sec, usize::MAX);
    assert_eq!(extended.st_atime.nsec, 123_456_789);
    assert_eq!(extended.st_mtime.sec, 2_147_483_648);
    assert_eq!(extended.st_mtime.nsec, 987_654_321);

    times = [
        TimeSpec {
            sec: usize::MAX - 2_147_483_648,
            nsec: 333_333_333,
        },
        TimeSpec {
            sec: 15_032_385_536,
            nsec: 666_666_666,
        },
    ];
    assert_eq!(utimens(PATH, &times), 0);
    let clamped = path_stat(PATH);
    assert_eq!(clamped.st_atime.sec as isize, -2_147_483_648);
    assert_eq!(clamped.st_atime.nsec, 0);
    assert_eq!(clamped.st_mtime.sec, 15_032_385_535);
    assert_eq!(clamped.st_mtime.nsec, 0);

    times = [
        TimeSpec { sec: 100, nsec: 0 },
        TimeSpec { sec: 200, nsec: 0 },
    ];
    assert_eq!(utimens(PATH, &times), 0);
    let baseline = path_stat(PATH);

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

    verify_nonowner_permissions(fd, 0o666, 0);
    verify_nonowner_permissions(fd, 0o000, -EACCES);

    assert_eq!(close(fd), 0);
    assert_eq!(unlink(PATH), 0);
    println!(
        "UTIMENS_SPECIAL PASS omit=pass now=pass invalid_nsec=pass negative_epoch_nsec=pass clamp=pass missing_double_omit=pass permission=pass"
    );
    0
}
