#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDWR, O_TRUNC, close, exit, fork, ftruncate, mmap, munmap, open, pwrite, unlink,
    waitpid,
};

const PATH: &str = "/tmp/respos_mmap_phase5.tmp\0";
const PAGE_SIZE: usize = 4096;
const MAP_SIZE: usize = 3 * PAGE_SIZE;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED: usize = 0x1;
const MAP_PRIVATE: usize = 0x2;
const SIGBUS: i32 = 7;

fn wait_child(pid: isize) -> Option<i32> {
    let mut status = -1;
    let result = waitpid(pid as usize, &mut status);
    if result != pid {
        println!("MMAP_PHASE5 waitpid pid={} result={}", pid, result);
        None
    } else {
        Some(status)
    }
}

fn expect_sigbus(address: usize, label: &str, name: &str) -> bool {
    let child = fork();
    if child < 0 {
        println!("MMAP_PHASE5 {} {} fork failed={}", label, name, child);
        return false;
    }
    if child == 0 {
        let value = unsafe { (address as *const u8).read_volatile() };
        core::hint::black_box(value);
        exit(99);
        unreachable!();
    }
    let Some(status) = wait_child(child) else {
        return false;
    };
    if status & 0x7f != SIGBUS {
        println!(
            "MMAP_PHASE5_EXPECTED_FAIL {} {} status={} expected_signal={}",
            label, name, status, SIGBUS
        );
        false
    } else {
        true
    }
}

fn expect_byte(address: usize, expected: u8, label: &str, name: &str) -> bool {
    let child = fork();
    if child < 0 {
        println!("MMAP_PHASE5 {} {} fork failed={}", label, name, child);
        return false;
    }
    if child == 0 {
        let actual = unsafe { (address as *const u8).read_volatile() };
        exit(if actual == expected { 0 } else { 98 });
        unreachable!();
    }
    let Some(status) = wait_child(child) else {
        return false;
    };
    if status != 0 {
        println!(
            "MMAP_PHASE5_EXPECTED_FAIL {} {} status={} expected_byte={:#x}",
            label, name, status, expected
        );
        false
    } else {
        true
    }
}

fn write_byte(fd: usize, offset: usize, value: u8) -> bool {
    pwrite(fd, &[value], offset as isize) == 1
}

fn test_mode(fd: usize, map_flag: usize, label: &str) -> bool {
    let mut ok = true;
    assert_eq!(ftruncate(fd, PAGE_SIZE + 128), 0);
    assert!(write_byte(fd, PAGE_SIZE + 64, 0x5a));
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, map_flag, fd as isize, 0);
    assert!(mapping > 0);
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 64,
        0x5a,
        label,
        "initial_data",
    );
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 512,
        0,
        label,
        "partial_eof_zero",
    );
    ok &= expect_sigbus(
        mapping as usize + 2 * PAGE_SIZE,
        label,
        "initial_beyond_eof",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);

    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    assert!(write_byte(fd, PAGE_SIZE + 512, 0x7c));
    assert!(write_byte(fd, 2 * PAGE_SIZE + 17, 0x6d));
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, map_flag, fd as isize, 0);
    assert!(mapping > 0);
    let partial_before =
        unsafe { ((mapping as usize + PAGE_SIZE + 512) as *const u8).read_volatile() };
    let full_before =
        unsafe { ((mapping as usize + 2 * PAGE_SIZE + 17) as *const u8).read_volatile() };
    assert_eq!(partial_before, 0x7c);
    assert_eq!(full_before, 0x6d);
    assert_eq!(ftruncate(fd, PAGE_SIZE + 128), 0);
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 512,
        0,
        label,
        "truncate_partial_zero",
    );
    ok &= expect_sigbus(
        mapping as usize + 2 * PAGE_SIZE + 17,
        label,
        "truncate_resident_beyond_eof",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);

    assert_eq!(ftruncate(fd, PAGE_SIZE), 0);
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, map_flag, fd as isize, 0);
    assert!(mapping > 0);
    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    assert!(write_byte(fd, 2 * PAGE_SIZE + 33, 0xa7));
    ok &= expect_byte(
        mapping as usize + 2 * PAGE_SIZE + 33,
        0xa7,
        label,
        "growth_dynamic_eof",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);

    if ok {
        println!("MMAP_PHASE5 {} PASS", label);
    }
    ok
}

fn test_private_cow_truncate(fd: usize) -> bool {
    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_PRIVATE, fd as isize, 0);
    assert!(mapping > 0);
    unsafe {
        ((mapping as usize + PAGE_SIZE + 64) as *mut u8).write_volatile(0x44);
        ((mapping as usize + PAGE_SIZE + 512) as *mut u8).write_volatile(0x55);
        ((mapping as usize + 2 * PAGE_SIZE + 17) as *mut u8).write_volatile(0x66);
    }
    assert_eq!(ftruncate(fd, PAGE_SIZE + 128), 0);
    let mut ok = true;
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 64,
        0x44,
        "private",
        "cow_retained_data",
    );
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 512,
        0x55,
        "private",
        "cow_partial_tail_retained",
    );
    ok &= expect_sigbus(
        mapping as usize + 2 * PAGE_SIZE + 17,
        "private",
        "cow_full_page_sigbus",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);
    if ok {
        println!("MMAP_PHASE5 private_cow_truncate PASS");
    }
    ok
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0);
    let fd = fd as usize;

    let shared_ok = test_mode(fd, MAP_SHARED, "shared");
    assert_eq!(ftruncate(fd, 0), 0);
    let private_ok = test_mode(fd, MAP_PRIVATE, "private");
    let private_cow_ok = test_private_cow_truncate(fd);

    assert_eq!(close(fd), 0);
    assert_eq!(unlink(PATH), 0);
    if shared_ok && private_ok && private_cow_ok {
        println!("MMAP_PHASE5 ALL PASS");
        0
    } else {
        println!("MMAP_PHASE5 CURRENT DIFFERENCES CONFIRMED");
        1
    }
}
