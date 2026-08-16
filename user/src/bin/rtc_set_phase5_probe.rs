#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CLOEXEC, O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, TimeSpec, clock_gettime_raw, clock_settime_raw,
    close, exec, exit, fork, fsync, ioctl, open, read, reboot_restart, setuid, sync, unlink,
    waitpid, write,
};

const RTC_PATH: &str = "/dev/rtc\0";
const RTC_RD_TIME: usize = 0x8024_7009;
const RTC_SET_TIME: usize = 0x4024_700a;
const CLOCK_REALTIME: usize = 0;
const RESET_STATE_PATH: &str = "/respos/rtc_set_phase5.reset\0";
const RESET_PERSIST: bool = option_env!("TASK_A_RTC_RESET_PERSIST_PROBE").is_some();
const ENOENT: isize = 2;
const EACCES: isize = 13;
const EFAULT: isize = 14;
const EINVAL: isize = 22;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RtcTime {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
}

fn read_rtc(fd: usize) -> RtcTime {
    let mut value = RtcTime::default();
    assert_eq!(
        ioctl(fd, RTC_RD_TIME, &mut value as *mut RtcTime as usize),
        0
    );
    value
}

fn set_rtc(fd: usize, value: &RtcTime) -> isize {
    ioctl(fd, RTC_SET_TIME, value as *const RtcTime as usize)
}

fn leap(year: usize) -> bool {
    year % 4 == 0 && year % 100 != 0 || year % 400 == 0
}

fn rtc_seconds(value: RtcTime) -> usize {
    let year = (value.tm_year + 1900) as usize;
    let month = value.tm_mon as usize;
    let month_days = [
        31usize,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let years_before = year - 1;
    let leap_before = years_before / 4 - years_before / 100 + years_before / 400;
    let epoch_base = 1969usize;
    let epoch_leap = epoch_base / 4 - epoch_base / 100 + epoch_base / 400;
    let days = (year - 1970) * 365 + leap_before - epoch_leap
        + month_days[..month].iter().sum::<usize>()
        + value.tm_mday as usize
        - 1;
    days * 86_400
        + value.tm_hour as usize * 3_600
        + value.tm_min as usize * 60
        + value.tm_sec as usize
}

fn realtime() -> TimeSpec {
    let mut value = TimeSpec::default();
    assert_eq!(clock_gettime_raw(CLOCK_REALTIME, &mut value), 0);
    value
}

fn time_us(value: TimeSpec) -> usize {
    value.sec * 1_000_000 + value.nsec / 1_000
}

fn reset_target() -> RtcTime {
    RtcTime {
        tm_sec: 20,
        tm_min: 34,
        tm_hour: 12,
        tm_mday: 15,
        tm_mon: 5,
        tm_year: 131,
        tm_wday: -1,
        tm_yday: -1,
        tm_isdst: -1,
    }
}

fn run_hwclock(path: &'static str) {
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        let argv = [
            path.as_ptr(),
            "hwclock\0".as_ptr(),
            "-r\0".as_ptr(),
            core::ptr::null(),
        ];
        let result = exec(path, &argv);
        println!("exec hwclock failed: {}", result);
        exit(-1);
    }
    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0, "{} hwclock -r failed", path);
}

fn run_reset_persistence(fd: usize) -> i32 {
    let state_fd = open(RESET_STATE_PATH, O_RDONLY | O_CLOEXEC, 0);
    if state_fd == -ENOENT {
        let original = read_rtc(fd);
        let state_fd = open(
            RESET_STATE_PATH,
            O_CREATE | O_TRUNC | O_RDWR | O_CLOEXEC,
            0o600,
        );
        assert!(state_fd >= 0);
        let state_fd = state_fd as usize;
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &original as *const RtcTime as *const u8,
                core::mem::size_of::<RtcTime>(),
            )
        };
        assert_eq!(write(state_fd, bytes), bytes.len() as isize);
        assert_eq!(fsync(state_fd), 0);
        assert_eq!(close(state_fd), 0);
        assert_eq!(sync(), 0);
        assert_eq!(set_rtc(fd, &reset_target()), 0);
        println!("RTC_RESET_PERSIST PREPARE PASS");
        let result = reboot_restart();
        panic!("reboot restart returned: {}", result);
    }
    assert!(state_fd >= 0, "open reset state failed: {}", state_fd);
    let state_fd = state_fd as usize;
    let mut original = RtcTime::default();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut original as *mut RtcTime as *mut u8,
            core::mem::size_of::<RtcTime>(),
        )
    };
    assert_eq!(read(state_fd, bytes), bytes.len() as isize);
    assert_eq!(close(state_fd), 0);

    let target = reset_target();
    let observed = read_rtc(fd);
    assert!(rtc_seconds(observed).abs_diff(rtc_seconds(target)) <= 2);
    let system = realtime();
    assert!(
        system.sec.abs_diff(rtc_seconds(target)) <= 2,
        "system realtime was not initialized from reset-persistent RTC"
    );
    assert_eq!(set_rtc(fd, &original), 0);
    assert_eq!(close(fd), 0);
    assert_eq!(unlink(RESET_STATE_PATH), 0);
    assert_eq!(sync(), 0);
    println!("RTC_RESET_PERSIST VERIFY PASS device_reset=pass boot_reinitialize=pass restore=pass");
    0
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let fd = open(RTC_PATH, O_RDWR | O_CLOEXEC, 0);
    assert!(fd >= 0, "open RTC failed: {}", fd);
    let fd = fd as usize;
    if RESET_PERSIST {
        return run_reset_persistence(fd);
    }
    let original_rtc = read_rtc(fd);
    let original_system = realtime();

    let shifted_system = TimeSpec {
        sec: original_system.sec + 3_600,
        nsec: original_system.nsec,
    };
    assert_eq!(clock_settime_raw(CLOCK_REALTIME, &shifted_system), 0);
    let after_system_set_rtc = read_rtc(fd);
    assert!(rtc_seconds(after_system_set_rtc).abs_diff(rtc_seconds(original_rtc)) <= 2);
    assert_eq!(clock_settime_raw(CLOCK_REALTIME, &original_system), 0);

    let target = reset_target();
    let system_before_rtc_set = realtime();
    assert_eq!(set_rtc(fd, &target), 0);
    let observed = read_rtc(fd);
    assert!(rtc_seconds(observed).abs_diff(rtc_seconds(target)) <= 1);
    let system_after_rtc_set = realtime();
    assert!(time_us(system_after_rtc_set).abs_diff(time_us(system_before_rtc_set)) < 2_000_000);

    let invalid = RtcTime {
        tm_mday: 30,
        tm_mon: 1,
        tm_year: 131,
        ..target
    };
    assert_eq!(set_rtc(fd, &invalid), -EINVAL);
    let after_invalid = read_rtc(fd);
    assert!(rtc_seconds(after_invalid).abs_diff(rtc_seconds(observed)) <= 1);
    assert_eq!(ioctl(fd, RTC_SET_TIME, usize::MAX), -EFAULT);

    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(setuid(65_534), 0);
        assert_eq!(set_rtc(fd, &target), -EACCES);
        assert_eq!(ioctl(fd, RTC_SET_TIME, usize::MAX), -EACCES);
        exit(0);
    }
    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);

    assert_eq!(set_rtc(fd, &original_rtc), 0);
    let restored = read_rtc(fd);
    assert!(rtc_seconds(restored).abs_diff(rtc_seconds(original_rtc)) <= 1);
    assert_eq!(close(fd), 0);
    run_hwclock("/musl/busybox\0");
    run_hwclock("/glibc/busybox\0");
    println!(
        "RTC_SET_PHASE5 PASS hardware_read_write=pass clock_domains=pass validation=pass permission=pass restore=pass hwclock=pass"
    );
    0
}
