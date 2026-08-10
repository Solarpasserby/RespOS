#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDWR, O_TRUNC, close, fgetxattr, fsetxattr, getxattr, link, listxattr, open,
    setxattr, unlink,
};

const PATH: &str = "/respos-xattr-probe\0";
const ALIAS: &str = "/respos-xattr-alias\0";
const NAME: &str = "user.respos\0";
const XATTR_CREATE: usize = 1;
const XATTR_REPLACE: usize = 2;

fn cleanup() {
    let _ = unlink(PATH);
    let _ = unlink(ALIAS);
}

fn read_path(path: &str) -> [u8; 7] {
    let mut value = [0u8; 7];
    assert_eq!(getxattr(path, NAME, &mut value), 7);
    value
}

fn prepare() {
    cleanup();
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o640);
    assert!(fd >= 0);
    assert_eq!(setxattr(PATH, NAME, b"persist", XATTR_CREATE), 0);
    assert_eq!(close(fd as usize), 0);
    println!("FS_XATTR_PREPARE_PASS");
}

fn verify() {
    assert_eq!(&read_path(PATH), b"persist");
    println!("FS_XATTR_PERSISTENCE_PASS");
}

fn normal() {
    cleanup();
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o640);
    assert!(fd >= 0);
    let fd = fd as usize;
    assert_eq!(setxattr(PATH, NAME, b"initial", XATTR_CREATE), 0);
    assert_eq!(setxattr(PATH, NAME, b"again!!", XATTR_CREATE), -17);
    assert_eq!(
        setxattr(PATH, "user.missing\0", b"value", XATTR_REPLACE),
        -61
    );
    assert_eq!(&read_path(PATH), b"initial");
    assert_eq!(link(PATH, ALIAS), 0);
    assert_eq!(&read_path(ALIAS), b"initial");
    assert_eq!(fsetxattr(fd, NAME, b"replace", XATTR_REPLACE), 0);
    assert_eq!(&read_path(ALIAS), b"replace");
    let mut list = [0u8; 64];
    let listed = listxattr(PATH, &mut list);
    assert!(listed > 0);
    assert!(
        list[..listed as usize]
            .windows(NAME.len())
            .any(|name| name == NAME.as_bytes())
    );
    assert_eq!(unlink(PATH), 0);
    assert_eq!(unlink(ALIAS), 0);
    let mut value = [0u8; 7];
    assert_eq!(fgetxattr(fd, NAME, &mut value), 7);
    assert_eq!(&value, b"replace");
    assert_eq!(close(fd), 0);
    println!("FS_XATTR_PROBE_PASS");
}

#[unsafe(no_mangle)]
fn main(argc: usize, argv: &[&str]) -> i32 {
    let mode = if argc > 1 { argv[1] } else { "normal" };
    match mode {
        "normal" => normal(),
        "prepare" => prepare(),
        "verify" => verify(),
        "cleanup" => cleanup(),
        _ => panic!("unknown mode"),
    }
    0
}
