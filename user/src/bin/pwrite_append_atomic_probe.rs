#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{
    O_APPEND, O_CREATE, O_RDWR, O_TRUNC, close, fork, ftruncate, mmap_raw, munmap, open, pread,
    pwrite, unlink, waitpid, yield_,
};

const PATH: &str = "/tmp/pwrite_append_atomic_probe\0";
const PAGE_SIZE: usize = 4096;
const RECORD_SIZE: usize = 128 * 1024;
const ROUNDS: usize = 16;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;
const MAP_PRIVATE_ANONYMOUS: usize = 0x2 | 0x20;

#[repr(C)]
struct Control {
    ready: AtomicU32,
    go: AtomicU32,
}

fn record_is(bytes: &[u8], value: u8) -> bool {
    bytes.iter().all(|byte| *byte == value)
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR | O_APPEND, 0o600);
    assert!(fd >= 0, "open failed: {}", fd);
    let fd = fd as usize;

    let control_addr = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    assert!(control_addr > 0, "control mmap failed: {}", control_addr);
    let control_addr = control_addr as usize;
    let control = unsafe { &*(control_addr as *const Control) };

    let buffers_len = RECORD_SIZE * 4;
    let buffers_addr = mmap_raw(
        0,
        buffers_len,
        PROT_READ_WRITE,
        MAP_PRIVATE_ANONYMOUS,
        -1,
        0,
    );
    assert!(buffers_addr > 0, "buffer mmap failed: {}", buffers_addr);
    let buffers_addr = buffers_addr as usize;
    let buffers = unsafe { core::slice::from_raw_parts_mut(buffers_addr as *mut u8, buffers_len) };
    let (first, rest) = buffers.split_at_mut(RECORD_SIZE);
    let (second, actual) = rest.split_at_mut(RECORD_SIZE);
    first.fill(0x41);
    second.fill(0x62);
    let mut interleaved = 0usize;

    for _round in 0..ROUNDS {
        assert_eq!(ftruncate(fd, 0), 0);
        control.ready.store(0, Ordering::Relaxed);
        control.go.store(0, Ordering::Relaxed);

        let child = fork();
        assert!(child >= 0, "fork failed: {}", child);
        if child == 0 {
            control.ready.store(1, Ordering::Release);
            while control.go.load(Ordering::Acquire) == 0 {
                let _ = yield_();
            }
            assert_eq!(pwrite(fd, first, 0), RECORD_SIZE as isize);
            return 0;
        }

        while control.ready.load(Ordering::Acquire) == 0 {
            let _ = yield_();
        }
        control.go.store(1, Ordering::Release);
        assert_eq!(pwrite(fd, second, 0), RECORD_SIZE as isize);

        let mut status = 0;
        assert_eq!(waitpid(child as usize, &mut status), child);
        assert_eq!(status, 0);
        assert_eq!(pread(fd, actual, 0), (RECORD_SIZE * 2) as isize);
        let first_then_second =
            record_is(&actual[..RECORD_SIZE], 0x41) && record_is(&actual[RECORD_SIZE..], 0x62);
        let second_then_first =
            record_is(&actual[..RECORD_SIZE], 0x62) && record_is(&actual[RECORD_SIZE..], 0x41);
        if !first_then_second && !second_then_first {
            interleaved += 1;
        }
    }

    assert_eq!(munmap(buffers_addr, buffers_len), 0);
    assert_eq!(munmap(control_addr, PAGE_SIZE), 0);
    assert_eq!(close(fd), 0);
    assert_eq!(unlink(PATH), 0);
    if interleaved != 0 {
        println!(
            "PWRITE_APPEND_ATOMIC_EXPECTED_FAIL interleaved={} rounds={}",
            interleaved, ROUNDS
        );
        return 1;
    }
    println!("PWRITE_APPEND_ATOMIC PASS rounds={}", ROUNDS);
    0
}
