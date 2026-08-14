#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AT_FDCWD, Stat, Statx, mknodat, setxattr, stat, statx, unlink};

const S_IFMT: u32 = 0o170000;
const S_IFIFO: u32 = 0o010000;
const S_IFCHR: u32 = 0o020000;
const S_IFBLK: u32 = 0o060000;
const S_IFSOCK: u32 = 0o140000;

const CHAR_PATH: &str = "/respos-mknod-char\0";
const BLOCK_PATH: &str = "/respos-mknod-block\0";
const FIFO_PATH: &str = "/respos-mknod-fifo\0";
const SOCKET_PATH: &str = "/respos-mknod-socket\0";
const XATTR_NAME: &str = "user.respos-special\0";

fn cleanup() {
    let _ = unlink(CHAR_PATH);
    let _ = unlink(BLOCK_PATH);
    let _ = unlink(FIFO_PATH);
    let _ = unlink(SOCKET_PATH);
}

fn check_node(path: &str, expected_type: u32, expected_rdev: u64) {
    let mut metadata = Stat::default();
    assert_eq!(stat(path, &mut metadata), 0);
    assert_eq!(metadata.st_mode & S_IFMT, expected_type);
    assert_eq!(metadata.st_rdev, expected_rdev);
    assert_eq!(setxattr(path, XATTR_NAME, b"x", 1), -1);
}

fn check_statx_device(path: &str, expected_major: u32, expected_minor: u32) {
    const STATX_BASIC_STATS: u32 = 0x0000_07ff;
    let mut metadata = Statx::default();
    assert_eq!(
        statx(AT_FDCWD, path, 0, STATX_BASIC_STATS, &mut metadata),
        0
    );
    assert_eq!(metadata.stx_rdev_major, expected_major);
    assert_eq!(metadata.stx_rdev_minor, expected_minor);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    const CHAR_MAJOR: u32 = 0xabc;
    const CHAR_MINOR: u32 = 0x54321;
    const CHAR_DEV: usize = (CHAR_MINOR as usize & 0xff)
        | ((CHAR_MAJOR as usize & 0xfff) << 8)
        | ((CHAR_MINOR as usize & !0xff) << 12);
    const BLOCK_DEV: usize = 0x700;

    assert_eq!(core::mem::size_of::<Statx>(), 256);
    cleanup();
    assert_eq!(
        mknodat(AT_FDCWD, CHAR_PATH, S_IFCHR as usize | 0o600, CHAR_DEV),
        0
    );
    assert_eq!(
        mknodat(AT_FDCWD, BLOCK_PATH, S_IFBLK as usize | 0o600, BLOCK_DEV,),
        0
    );
    assert_eq!(mknodat(AT_FDCWD, FIFO_PATH, S_IFIFO as usize | 0o600, 0), 0);
    assert_eq!(
        mknodat(AT_FDCWD, SOCKET_PATH, S_IFSOCK as usize | 0o600, 0),
        0
    );

    check_node(CHAR_PATH, S_IFCHR, CHAR_DEV as u64);
    check_statx_device(CHAR_PATH, CHAR_MAJOR, CHAR_MINOR);
    check_node(BLOCK_PATH, S_IFBLK, BLOCK_DEV as u64);
    check_statx_device(BLOCK_PATH, 7, 0);
    check_node(FIFO_PATH, S_IFIFO, 0);
    check_node(SOCKET_PATH, S_IFSOCK, 0);
    cleanup();
    println!("MKNOD_XATTR_PROBE_PASS");
    0
}
