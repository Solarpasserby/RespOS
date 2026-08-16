#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    clock_gettime_raw, clock_settime_raw, close, fsync, getdents64, lseek, mkdir, mount, nanosleep,
    open, read, rmdir, stat, sync, unlink, utimens, write, Stat, TimeSpec, O_CLOEXEC, O_CREATE,
    O_DIRECTORY, O_NOATIME, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY,
};

const PATH: &str = "/respos/atime_phase5.tmp\0";
const EVICT_PATH: &str = "/respos/atime_phase5.evict\0";
const DIR_PATH: &str = "/respos/atime_phase5.dir\0";
const MOUNT_POINT: &str = "/respos\0";
const NONE: &str = "none\0";
const EXT4: &str = "ext4\0";
const PERF_PATH: &str = "/proc/respos_perf\0";
const DIRTYTIME_EXPIRE_PATH: &str = "/proc/sys/vm/dirtytime_expire_seconds\0";
const CLOCK_REALTIME: usize = 0;
const SEEK_SET: usize = 0;
const MS_REMOUNT: usize = 32;
const MS_NOATIME: usize = 1024;
const MS_NODIRATIME: usize = 2048;
const MS_STRICTATIME: usize = 1 << 24;
const MS_LAZYTIME: usize = 1 << 25;

fn path_stat(path: &str) -> Stat {
    let mut value = Stat::default();
    assert_eq!(stat(path, &mut value), 0);
    value
}

fn realtime() -> TimeSpec {
    let mut value = TimeSpec::default();
    assert_eq!(clock_gettime_raw(CLOCK_REALTIME, &mut value), 0);
    value
}

fn set_times(path: &str, atime_sec: usize, mtime_sec: usize) {
    let times = [
        TimeSpec {
            sec: atime_sec,
            nsec: 111_222_333,
        },
        TimeSpec {
            sec: mtime_sec,
            nsec: 444_555_666,
        },
    ];
    assert_eq!(utimens(path, &times), 0);
}

fn read_byte(fd: usize) {
    let mut byte = [0u8; 1];
    assert_eq!(lseek(fd, 0, SEEK_SET), 0);
    assert_eq!(read(fd, &mut byte), 1);
}

fn read_directory(fd: usize) {
    let mut buf = [0u8; 512];
    assert_eq!(lseek(fd, 0, SEEK_SET), 0);
    assert!(getdents64(fd, &mut buf) > 0);
}

fn remount(flags: usize) {
    assert_eq!(mount(NONE, MOUNT_POINT, EXT4, MS_REMOUNT | flags, 0), 0);
}

fn perf_atime_updates() -> Option<usize> {
    let fd = open(PERF_PATH, O_RDWR | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    assert_eq!(lseek(fd, 0, SEEK_SET), 0);
    let mut buf = [0u8; 16 * 1024];
    let len = read(fd, &mut buf);
    assert!(len > 0);
    assert_eq!(close(fd), 0);
    let bytes = &buf[..len as usize];
    if bytes
        .windows(b"enabled=0".len())
        .any(|part| part == b"enabled=0")
    {
        return None;
    }
    let key = b"atime_updates=";
    let start = bytes
        .windows(key.len())
        .position(|part| part == key)
        .expect("missing ext4 atime counter")
        + key.len();
    let mut value = 0usize;
    let mut found = false;
    for &byte in &bytes[start..] {
        if !byte.is_ascii_digit() {
            break;
        }
        found = true;
        value = value * 10 + (byte - b'0') as usize;
    }
    assert!(found);
    Some(value)
}

fn reset_perf() {
    let fd = open(PERF_PATH, O_RDWR | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    assert_eq!(write(fd, b"reset\n"), 6);
    assert_eq!(close(fd), 0);
}

fn drop_dentry_cache() {
    let command = b"drop_dentry_cache\n";
    let fd = open(PERF_PATH, O_RDWR | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    assert_eq!(write(fd, command), command.len() as isize);
    assert_eq!(close(fd), 0);
}

fn read_usize_sysctl(path: &str) -> usize {
    let fd = open(path, O_RDONLY | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    let mut buf = [0u8; 32];
    let len = read(fd, &mut buf);
    assert!(len > 0);
    assert_eq!(close(fd), 0);
    let text = core::str::from_utf8(&buf[..len as usize]).expect("sysctl is not UTF-8");
    text.trim().parse::<usize>().expect("invalid usize sysctl")
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

fn sleep_for_background_writeback() {
    let request = TimeSpec {
        sec: 1,
        nsec: 200_000_000,
    };
    let mut remaining = TimeSpec::default();
    assert_eq!(nanosleep(&request, &mut remaining), 0);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(PATH);
    let _ = unlink(EVICT_PATH);
    let _ = rmdir(DIR_PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0);
    let fd = fd as usize;
    assert_eq!(write(fd, b"x"), 1);

    remount(0);
    let now = realtime().sec;
    set_times(PATH, now + 3600, now - 3600);
    let before = path_stat(PATH);
    read_byte(fd);
    let after = path_stat(PATH);
    assert_eq!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);

    set_times(PATH, 100, 200);
    let before = path_stat(PATH);
    let read_started = realtime();
    read_byte(fd);
    let after = path_stat(PATH);
    assert!(after.st_atime.sec >= read_started.sec.saturating_sub(1));
    assert_eq!(after.st_ctime, before.st_ctime);
    read_byte(fd);
    let repeated = path_stat(PATH);
    assert_eq!(repeated.st_atime, after.st_atime);
    assert_eq!(repeated.st_ctime, after.st_ctime);

    // Isolate relatime's 24-hour rule: before shifting realtime, atime is
    // newer than both mtime and ctime, so neither ordering rule can trigger.
    let original_clock = realtime();
    set_times(PATH, original_clock.sec + 3600, original_clock.sec);
    let before = path_stat(PATH);
    assert!(before.st_atime.sec > before.st_mtime.sec && before.st_atime.sec > before.st_ctime.sec);
    let shifted_clock = TimeSpec {
        sec: original_clock.sec + 3600 + 24 * 60 * 60,
        nsec: original_clock.nsec,
    };
    assert_eq!(clock_settime_raw(CLOCK_REALTIME, &shifted_clock), 0);
    read_byte(fd);
    let after = path_stat(PATH);
    assert!(after.st_atime.sec >= shifted_clock.sec.saturating_sub(1));
    assert_eq!(after.st_ctime, before.st_ctime);
    assert_eq!(clock_settime_raw(CLOCK_REALTIME, &original_clock), 0);

    // lazytime must publish atime immediately but avoid lower metadata I/O
    // until a durability boundary. The counter assertion is active in the
    // perf_counters diagnostic build; the observable contract runs always.
    remount(MS_LAZYTIME);
    set_times(PATH, 100, 200);
    assert_eq!(fsync(fd), 0);
    reset_perf();
    let before = path_stat(PATH);
    read_byte(fd);
    let after = path_stat(PATH);
    assert_ne!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);
    if let Some(updates) = perf_atime_updates() {
        assert_eq!(updates, 0);
    }
    assert_eq!(fsync(fd), 0);
    if let Some(updates) = perf_atime_updates() {
        assert_eq!(updates, 1);
    }

    set_times(PATH, 100, 200);
    assert_eq!(fsync(fd), 0);
    reset_perf();
    read_byte(fd);
    assert_eq!(sync(), 0);
    if let Some(updates) = perf_atime_updates() {
        assert_eq!(updates, 1);
    }

    // Linux exposes dirtytime expiry as a monotonic background policy. Zero
    // disables periodic work; changing it to a non-zero value wakes the
    // worker and re-evaluates already-pending timestamps.
    let original_dirtytime_expire = read_usize_sysctl(DIRTYTIME_EXPIRE_PATH);
    assert!(original_dirtytime_expire > 0);
    write_usize_sysctl(DIRTYTIME_EXPIRE_PATH, 0);
    set_times(PATH, 100, 200);
    assert_eq!(fsync(fd), 0);
    reset_perf();
    read_byte(fd);
    sleep_for_background_writeback();
    if let Some(updates) = perf_atime_updates() {
        assert_eq!(updates, 0);
    }
    write_usize_sysctl(DIRTYTIME_EXPIRE_PATH, 1);
    sleep_for_background_writeback();
    if let Some(updates) = perf_atime_updates() {
        assert_eq!(updates, 1);
    }
    write_usize_sysctl(DIRTYTIME_EXPIRE_PATH, original_dirtytime_expire);

    // A cache-pressure eviction is an independent durability boundary. Keep
    // periodic expiry disabled, remove the final dentry/file owner, and force
    // the diagnostic dentry reclaim path. A fresh lookup must read the new
    // atime from the lower inode rather than the old in-memory object.
    write_usize_sysctl(DIRTYTIME_EXPIRE_PATH, 0);
    let evict_fd = open(EVICT_PATH, O_CREATE | O_TRUNC | O_RDWR | O_CLOEXEC, 0o600);
    assert!(evict_fd >= 0);
    let evict_fd = evict_fd as usize;
    assert_eq!(write(evict_fd, b"e"), 1);
    assert_eq!(fsync(evict_fd), 0);
    set_times(EVICT_PATH, 100, 200);
    assert_eq!(fsync(evict_fd), 0);
    reset_perf();
    read_byte(evict_fd);
    let visible_before_evict = path_stat(EVICT_PATH);
    assert!(visible_before_evict.st_atime.sec > 100);
    assert_eq!(close(evict_fd), 0);
    drop_dentry_cache();
    if let Some(updates) = perf_atime_updates() {
        assert_eq!(updates, 1);
    }
    let reloaded_after_evict = path_stat(EVICT_PATH);
    assert_eq!(reloaded_after_evict.st_atime, visible_before_evict.st_atime);
    assert_eq!(unlink(EVICT_PATH), 0);
    write_usize_sysctl(DIRTYTIME_EXPIRE_PATH, original_dirtytime_expire);
    remount(0);

    remount(MS_STRICTATIME);
    let now = realtime().sec;
    set_times(PATH, now + 3600, now - 3600);
    let before = path_stat(PATH);
    read_byte(fd);
    let after = path_stat(PATH);
    assert_ne!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);

    remount(MS_NOATIME);
    set_times(PATH, 100, 200);
    let before = path_stat(PATH);
    read_byte(fd);
    let after = path_stat(PATH);
    assert_eq!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);

    remount(0);
    assert_eq!(close(fd), 0);
    set_times(PATH, 100, 200);
    let before = path_stat(PATH);
    let fd = open(PATH, O_RDONLY | O_NOATIME | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    read_byte(fd);
    let after = path_stat(PATH);
    assert_eq!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);
    assert_eq!(close(fd), 0);

    assert_eq!(mkdir(DIR_PATH, 0o700), 0);
    let dir_fd = open(DIR_PATH, O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0);
    assert!(dir_fd >= 0);
    let dir_fd = dir_fd as usize;

    remount(0);
    let now = realtime().sec;
    set_times(DIR_PATH, now + 3600, now - 3600);
    let before = path_stat(DIR_PATH);
    read_directory(dir_fd);
    let after = path_stat(DIR_PATH);
    assert_eq!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);

    set_times(DIR_PATH, 100, 200);
    let before = path_stat(DIR_PATH);
    let read_started = realtime();
    read_directory(dir_fd);
    let after = path_stat(DIR_PATH);
    assert!(after.st_atime.sec >= read_started.sec.saturating_sub(1));
    assert_eq!(after.st_ctime, before.st_ctime);
    read_directory(dir_fd);
    let repeated = path_stat(DIR_PATH);
    assert_eq!(repeated.st_atime, after.st_atime);
    assert_eq!(repeated.st_ctime, after.st_ctime);

    remount(MS_STRICTATIME);
    let now = realtime().sec;
    set_times(DIR_PATH, now + 3600, now - 3600);
    let before = path_stat(DIR_PATH);
    read_directory(dir_fd);
    let after = path_stat(DIR_PATH);
    assert_ne!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);

    remount(MS_NODIRATIME);
    set_times(DIR_PATH, 100, 200);
    let before = path_stat(DIR_PATH);
    read_directory(dir_fd);
    let after = path_stat(DIR_PATH);
    assert_eq!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);

    // MS_NODIRATIME applies only to directories, not regular files.
    set_times(PATH, 100, 200);
    let before = path_stat(PATH);
    let regular_fd = open(PATH, O_RDONLY | O_CLOEXEC, 0);
    assert!(regular_fd >= 0);
    let regular_fd = regular_fd as usize;
    read_byte(regular_fd);
    let after = path_stat(PATH);
    assert_ne!(after.st_atime, before.st_atime);
    assert_eq!(after.st_ctime, before.st_ctime);
    assert_eq!(close(regular_fd), 0);

    assert_eq!(close(dir_fd), 0);
    assert_eq!(rmdir(DIR_PATH), 0);
    remount(0);
    assert_eq!(unlink(PATH), 0);

    println!(
        "ATIME_PHASE5 PASS relatime=pass relatime_24h=pass repeated=pass strictatime=pass mount_noatime=pass open_noatime=pass directory=pass nodiratime=pass lazytime=pass lazytime_background=pass lazytime_eviction=pass ctime=pass"
    );
    0
}
