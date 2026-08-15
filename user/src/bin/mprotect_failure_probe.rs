#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, close, exit, fork, ftruncate, mmap, mmap_raw, mprotect,
    munmap, open, unlink, waitpid,
};

const PATH: &str = "/tmp/respos_mprotect_failure.tmp\0";
const PAGE_SIZE: usize = 4096;
const PROT_NONE: usize = 0;
const PROT_READ: usize = 0x1;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const INVALID_PROT: usize = 0x4000_0000;
const MAP_SHARED: usize = 0x1;
const MAP_PRIVATE_ANONYMOUS: usize = 0x2 | 0x20;
const EACCES: isize = 13;
const ENOMEM: isize = 12;
const EINVAL: isize = 22;
const SIGSEGV: i32 = 11;

fn expect_write(address: usize, expected_signal: i32) {
    let child = fork();
    assert!(child >= 0, "fork failed: {}", child);
    if child == 0 {
        unsafe { (address as *mut u8).write_volatile(0x6d) };
        exit(0);
        unreachable!();
    }

    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    if expected_signal == 0 {
        assert_eq!(status, 0, "write child status={}", status);
    } else {
        assert_eq!(status & 0x7f, expected_signal);
    }
}

fn verify_einval_preserves_permissions() {
    let mapping = mmap_raw(
        0,
        2 * PAGE_SIZE,
        PROT_READ_WRITE,
        MAP_PRIVATE_ANONYMOUS,
        -1,
        0,
    );
    assert!(mapping > 0, "anonymous mmap failed: {}", mapping);
    let mapping = mapping as usize;
    unsafe {
        (mapping as *mut u8).write_volatile(0x11);
        ((mapping + PAGE_SIZE) as *mut u8).write_volatile(0x22);
    }
    assert_eq!(mprotect(mapping, PAGE_SIZE, PROT_READ), 0);

    assert_eq!(
        mprotect(mapping, 2 * PAGE_SIZE, PROT_READ_WRITE | INVALID_PROT),
        -EINVAL
    );
    expect_write(mapping, SIGSEGV);
    expect_write(mapping + PAGE_SIZE, 0);

    assert_eq!(mprotect(mapping + 1, PAGE_SIZE, PROT_NONE), -EINVAL);
    expect_write(mapping, SIGSEGV);
    expect_write(mapping + PAGE_SIZE, 0);
    assert_eq!(munmap(mapping, 2 * PAGE_SIZE), 0);
}

fn verify_eacces_does_not_grant_write() {
    let _ = unlink(PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0, "create failed: {}", fd);
    assert_eq!(ftruncate(fd as usize, PAGE_SIZE), 0);
    assert_eq!(close(fd as usize), 0);

    let fd = open(PATH, O_RDONLY, 0);
    assert!(fd >= 0, "read-only open failed: {}", fd);
    let mapping = mmap(0, PAGE_SIZE, PROT_READ, MAP_SHARED, fd, 0);
    assert!(mapping > 0, "file mmap failed: {}", mapping);
    assert_eq!(
        mprotect(mapping as usize, PAGE_SIZE, PROT_READ_WRITE),
        -EACCES
    );
    expect_write(mapping as usize, SIGSEGV);

    assert_eq!(munmap(mapping as usize, PAGE_SIZE), 0);
    assert_eq!(close(fd as usize), 0);
    assert_eq!(unlink(PATH), 0);
}

fn verify_unmapped_hole_errno() {
    let mapping = mmap_raw(
        0,
        3 * PAGE_SIZE,
        PROT_READ_WRITE,
        MAP_PRIVATE_ANONYMOUS,
        -1,
        0,
    );
    assert!(mapping > 0, "hole mmap failed: {}", mapping);
    let mapping = mapping as usize;
    assert_eq!(munmap(mapping + PAGE_SIZE, PAGE_SIZE), 0);
    assert_eq!(mprotect(mapping, 3 * PAGE_SIZE, PROT_READ), -ENOMEM);
    assert_eq!(munmap(mapping, PAGE_SIZE), 0);
    assert_eq!(munmap(mapping + 2 * PAGE_SIZE, PAGE_SIZE), 0);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    verify_einval_preserves_permissions();
    verify_eacces_does_not_grant_write();
    verify_unmapped_hole_errno();
    println!("MPROTECT_FAILURE PASS einval_atomic=pass eacces_write=pass hole_enomem=pass");
    0
}
