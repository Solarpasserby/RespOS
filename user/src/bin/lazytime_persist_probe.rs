#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    clock_settime_raw, close, fsync, lseek, mount, nanosleep, open, read, stat, sync, unlink,
    utimens, write, Stat, TimeSpec, O_CLOEXEC, O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY,
};

const PATH: &str = "/respos/lazytime_background.persist\0";
const MOUNT_POINT: &str = "/respos\0";
const NONE: &str = "none\0";
const EXT4: &str = "ext4\0";
const DIRTYTIME_EXPIRE_PATH: &str = "/proc/sys/vm/dirtytime_expire_seconds\0";
const CLOCK_REALTIME: usize = 0;
const SEEK_SET: usize = 0;
const MS_REMOUNT: usize = 32;
const MS_LAZYTIME: usize = 1 << 25;
const ENOENT: isize = 2;
const TARGET_ATIME_SEC: usize = 2_000_000_000;

fn file_stat() -> Result<Stat, isize> {
    let mut value = Stat::default();
    let result = stat(PATH, &mut value);
    if result == 0 {
        Ok(value)
    } else {
        Err(result)
    }
}

fn set_times(atime_sec: usize, mtime_sec: usize) {
    let times = [
        TimeSpec {
            sec: atime_sec,
            nsec: 0,
        },
        TimeSpec {
            sec: mtime_sec,
            nsec: 0,
        },
    ];
    assert_eq!(utimens(PATH, &times), 0);
}

fn read_usize_sysctl(path: &str) -> usize {
    let fd = open(path, O_RDONLY | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    let mut buf = [0u8; 32];
    let len = read(fd, &mut buf);
    assert!(len > 0);
    assert_eq!(close(fd), 0);
    core::str::from_utf8(&buf[..len as usize])
        .expect("sysctl is not UTF-8")
        .trim()
        .parse::<usize>()
        .expect("invalid usize sysctl")
}

fn write_usize_sysctl(path: &str, mut value: usize) {
    let mut digits = [0u8; 32];
    let mut start = digits.len() - 1;
    digits[start] = b'\n';
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let fd = open(path, O_WRONLY | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    assert_eq!(write(fd, &digits[start..]), (digits.len() - start) as isize);
    assert_eq!(close(fd), 0);
}

fn sleep_seconds(seconds: usize) {
    let request = TimeSpec {
        sec: seconds,
        nsec: 0,
    };
    let mut remaining = TimeSpec::default();
    assert_eq!(nanosleep(&request, &mut remaining), 0);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    match file_stat() {
        Err(error) if error == -ENOENT => {
            let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR | O_CLOEXEC, 0o600);
            assert!(fd >= 0);
            let fd = fd as usize;
            assert_eq!(write(fd, b"x"), 1);
            assert_eq!(fsync(fd), 0);

            assert_eq!(
                mount(NONE, MOUNT_POINT, EXT4, MS_REMOUNT | MS_LAZYTIME, 0,),
                0
            );
            set_times(100, 200);
            assert_eq!(fsync(fd), 0);
            let original_expire = read_usize_sysctl(DIRTYTIME_EXPIRE_PATH);
            write_usize_sysctl(DIRTYTIME_EXPIRE_PATH, 1);
            let target = TimeSpec {
                sec: TARGET_ATIME_SEC,
                nsec: 0,
            };
            assert_eq!(clock_settime_raw(CLOCK_REALTIME, &target), 0);
            assert_eq!(lseek(fd, 0, SEEK_SET), 0);
            let mut byte = [0u8; 1];
            assert_eq!(read(fd, &mut byte), 1);
            sleep_seconds(2);
            let visible = file_stat().expect("stat after background writeback failed");
            assert!(
                visible.st_atime.sec >= TARGET_ATIME_SEC
                    && visible.st_atime.sec <= TARGET_ATIME_SEC + 5
            );
            write_usize_sysctl(DIRTYTIME_EXPIRE_PATH, original_expire);
            assert_eq!(close(fd), 0);
            println!(
                "LAZYTIME_CRASH_IMAGE PREPARE PASS background_without_fsync=pass atime_sec={}",
                visible.st_atime.sec
            );
            // The host deliberately terminates QEMU after the marker. Do not
            // call sync, fsync, unmount, reboot, or the normal poweroff path.
            loop {
                sleep_seconds(60);
            }
        }
        Ok(value) => {
            assert!(
                value.st_atime.sec >= TARGET_ATIME_SEC
                    && value.st_atime.sec <= TARGET_ATIME_SEC + 5
            );
            assert_eq!(unlink(PATH), 0);
            assert_eq!(sync(), 0);
            println!(
                "LAZYTIME_CRASH_IMAGE VERIFY PASS persisted_background_atime=pass atime_sec={}",
                value.st_atime.sec
            );
            0
        }
        Err(error) => panic!("lazytime persistence stat failed: {}", error),
    }
}
