#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDWR, O_TRUNC, Stat, TimeSpec, close, fsync, open, stat, sync, unlink, utimens,
};

const PATH: &str = "/respos/ext4_timestamp_phase5.persist\0";
const AUTO_PATH: &str = "/respos/ext4_timestamp_phase5.auto\0";
const CLAMP_PATH: &str = "/respos/ext4_timestamp_phase5.clamp\0";
const LEGACY_PATH: &str = "/respos/ext4_timestamp_phase5.legacy\0";
const ENOENT: isize = 2;
const CALENDAR_EPOCH_FLOOR: usize = 1_704_067_200;
const LEGACY_LAYOUT: bool = option_env!("TASK_A_EXT4_LEGACY_LAYOUT").is_some();

fn verify(value: &Stat) {
    assert_eq!(value.st_atime.sec, usize::MAX);
    assert_eq!(value.st_atime.nsec, 123_456_789);
    assert_eq!(value.st_mtime.sec, 2_147_483_648);
    assert_eq!(value.st_mtime.nsec, 987_654_321);
}

fn verify_automatic_times(value: &Stat) {
    for time in [value.st_atime, value.st_mtime, value.st_ctime] {
        assert!(time.sec >= CALENDAR_EPOCH_FLOOR);
        assert!(time.nsec < 1_000_000_000);
    }
}

fn verify_clamped_times(value: &Stat) {
    assert_eq!(value.st_atime.sec as isize, -2_147_483_648);
    assert_eq!(value.st_atime.nsec, 0);
    assert_eq!(value.st_mtime.sec, 15_032_385_535);
    assert_eq!(value.st_mtime.nsec, 0);
}

fn verify_legacy_clamp(value: &Stat) {
    assert_eq!(value.st_atime.sec as isize, -2_147_483_648);
    assert_eq!(value.st_atime.nsec, 0);
    assert_eq!(value.st_mtime.sec, 2_147_483_647);
    assert_eq!(value.st_mtime.nsec, 0);
    assert!(value.st_ctime.sec >= CALENDAR_EPOCH_FLOOR);
    assert_eq!(value.st_ctime.nsec, 0);
}

fn run_legacy_layout() -> i32 {
    let mut value = Stat::default();
    let stat_result = stat(LEGACY_PATH, &mut value);
    if stat_result == -ENOENT {
        let fd = open(LEGACY_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
        assert!(fd >= 0, "legacy timestamp file create failed: {}", fd);
        let fd = fd as usize;
        assert_eq!(stat(LEGACY_PATH, &mut value), 0);
        verify_automatic_times(&value);
        assert_eq!(value.st_atime.nsec, 0);
        assert_eq!(value.st_mtime.nsec, 0);
        assert_eq!(value.st_ctime.nsec, 0);

        let times = [
            TimeSpec {
                sec: usize::MAX - 2_147_483_648,
                nsec: 333_333_333,
            },
            TimeSpec {
                sec: 2_147_483_648,
                nsec: 666_666_666,
            },
        ];
        assert_eq!(utimens(LEGACY_PATH, &times), 0);
        assert_eq!(stat(LEGACY_PATH, &mut value), 0);
        verify_legacy_clamp(&value);
        assert_eq!(fsync(fd), 0);
        assert_eq!(close(fd), 0);
        assert_eq!(sync(), 0);
        println!(
            "EXT4_TIMESTAMP_LEGACY PREPARE PASS signed32_clamp=pass seconds_granularity=pass automatic_realtime=pass"
        );
        return 0;
    }

    assert_eq!(
        stat_result, 0,
        "legacy timestamp stat failed: {}",
        stat_result
    );
    verify_legacy_clamp(&value);
    assert_eq!(unlink(LEGACY_PATH), 0);
    assert_eq!(sync(), 0);
    println!(
        "EXT4_TIMESTAMP_LEGACY VERIFY PASS signed32_clamp=pass seconds_granularity=pass automatic_realtime=pass"
    );
    0
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    if LEGACY_LAYOUT {
        return run_legacy_layout();
    }
    let mut value = Stat::default();
    let stat_result = stat(PATH, &mut value);
    if stat_result == -ENOENT {
        let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
        assert!(fd >= 0, "persistent file create failed: {}", fd);
        let fd = fd as usize;
        let times = [
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
        assert_eq!(stat(PATH, &mut value), 0);
        verify(&value);
        assert_eq!(fsync(fd), 0);
        assert_eq!(close(fd), 0);
        let auto_fd = open(AUTO_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
        assert!(
            auto_fd >= 0,
            "automatic-time file create failed: {}",
            auto_fd
        );
        let auto_fd = auto_fd as usize;
        assert_eq!(stat(AUTO_PATH, &mut value), 0);
        verify_automatic_times(&value);
        assert_eq!(fsync(auto_fd), 0);
        assert_eq!(close(auto_fd), 0);
        let clamp_fd = open(CLAMP_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
        assert!(clamp_fd >= 0, "clamp file create failed: {}", clamp_fd);
        let clamp_fd = clamp_fd as usize;
        let clamp_times = [
            TimeSpec {
                sec: usize::MAX - 2_147_483_648,
                nsec: 333_333_333,
            },
            TimeSpec {
                sec: 15_032_385_536,
                nsec: 666_666_666,
            },
        ];
        assert_eq!(utimens(CLAMP_PATH, &clamp_times), 0);
        assert_eq!(stat(CLAMP_PATH, &mut value), 0);
        verify_clamped_times(&value);
        assert_eq!(fsync(clamp_fd), 0);
        assert_eq!(close(clamp_fd), 0);
        assert_eq!(sync(), 0);
        println!("EXT4_TIMESTAMP_PERSIST PREPARE PASS");
        return 0;
    }

    assert_eq!(
        stat_result, 0,
        "persistent file stat failed: {}",
        stat_result
    );
    verify(&value);
    assert_eq!(stat(AUTO_PATH, &mut value), 0);
    verify_automatic_times(&value);
    assert_eq!(stat(CLAMP_PATH, &mut value), 0);
    verify_clamped_times(&value);
    assert_eq!(unlink(PATH), 0);
    assert_eq!(unlink(AUTO_PATH), 0);
    assert_eq!(unlink(CLAMP_PATH), 0);
    assert_eq!(sync(), 0);
    println!(
        "EXT4_TIMESTAMP_PERSIST VERIFY PASS negative_sec=pass epoch=pass nsec=pass clamp=pass automatic_realtime=pass"
    );
    0
}
