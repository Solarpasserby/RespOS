#![no_std]
#![no_main]
#![cfg_attr(target_arch = "loongarch64", allow(dead_code, unused_imports))]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, close, fork, mmap, mmap_raw, mprotect, munmap,
    open, pread, waitpid, write,
};

const PATH: &str = "/respos-buildstorm-private-map.bin\0";
const PERF_PATH: &str = "/proc/respos_perf\0";
const PAGE_SIZE: usize = 4096;
const FILE_SIZE: usize = 64 * 1024 * 1024;
const WORKERS: usize = 4;
const PROT_READ: usize = 0x1;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_PRIVATE: usize = 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;

fn prepare_file() {
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o644);
    assert!(fd >= 0, "probe file create failed: {}", fd);
    let fd = fd as usize;
    let mut page = [0u8; PAGE_SIZE];
    for (index, byte) in page.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
    }
    for _ in 0..FILE_SIZE / PAGE_SIZE {
        assert_eq!(write(fd, &page), PAGE_SIZE as isize);
    }
    assert_eq!(close(fd), 0);
}

fn reset_perf_counters() {
    let fd = open(PERF_PATH, O_WRONLY, 0);
    assert!(fd >= 0, "open respos_perf failed: {}", fd);
    let fd = fd as usize;
    assert_eq!(write(fd, b"reset\n"), 6);
    assert_eq!(close(fd), 0);
}

fn verify_mprotect_private_copy() {
    let fd = open(PATH, O_RDONLY, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    let base = mmap(0, PAGE_SIZE, PROT_READ, MAP_PRIVATE, fd as isize, 0);
    assert!(base > 0);
    assert_eq!(mprotect(base as usize, PAGE_SIZE, PROT_READ_WRITE), 0);
    unsafe { (base as *mut u8).write_volatile(0xe7) };
    assert_eq!(munmap(base as usize, PAGE_SIZE), 0);

    let mut byte = [0u8; 1];
    assert_eq!(pread(fd, &mut byte, 0), 1);
    assert_eq!(byte[0], 11, "private mprotect write reached backing file");
    assert_eq!(close(fd), 0);
}

fn touch_private_mapping(barrier: *mut AtomicUsize) -> i32 {
    unsafe { &*barrier }.fetch_add(1, Ordering::AcqRel);
    while unsafe { &*barrier }.load(Ordering::Acquire) <= WORKERS {
        core::hint::spin_loop();
    }

    let fd = open(PATH, O_RDONLY, 0);
    assert!(fd >= 0, "probe file open failed: {}", fd);
    let fd = fd as usize;
    let base = mmap(0, FILE_SIZE, PROT_READ, MAP_PRIVATE, fd as isize, 0);
    assert!(base > 0, "private mmap failed: {}", base);
    assert_eq!(close(fd), 0);

    let mut checksum = 0usize;
    for offset in (0..FILE_SIZE).step_by(PAGE_SIZE) {
        checksum = checksum.wrapping_add(unsafe {
            ((base as usize + offset) as *const u8).read_volatile() as usize
        });
    }
    assert_eq!(checksum, 11 * (FILE_SIZE / PAGE_SIZE));
    0
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
fn main() -> i32 {
    prepare_file();
    verify_mprotect_private_copy();

    let barrier = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    assert!(barrier > 0);
    let barrier = barrier as *mut AtomicUsize;
    unsafe { barrier.write(AtomicUsize::new(0)) };

    reset_perf_counters();
    let mut children = [0usize; WORKERS];
    for child in &mut children {
        let pid = fork();
        assert!(pid >= 0, "fork failed: {}", pid);
        if pid == 0 {
            return touch_private_mapping(barrier);
        }
        *child = pid as usize;
    }

    while unsafe { &*barrier }.load(Ordering::Acquire) != WORKERS {
        core::hint::spin_loop();
    }
    unsafe { &*barrier }.fetch_add(1, Ordering::Release);

    for pid in children {
        let mut status = -1;
        assert_eq!(waitpid(pid, &mut status), pid as isize);
        assert_eq!(status, 0);
    }

    println!(
        "BUILDSTORM_PRIVATE_MAP_PROBE_PASS file_mb={} workers={}",
        FILE_SIZE / 1024 / 1024,
        WORKERS
    );
    0
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "loongarch64")]
fn main() -> i32 {
    println!("buildstorm_private_map_probe skipped: RV64 diagnostic only");
    0
}
