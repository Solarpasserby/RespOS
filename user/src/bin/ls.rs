// user/src/bin/ls.rs

#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::string::String;
use alloc::vec;
use core::str;
use user_lib::{O_DIRECTORY, O_RDONLY, close, getdents64, open};

const DIRENT64_HEADER_SIZE: usize = 19;
const BUF_SIZE: usize = 8192;

fn read_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

fn nul_terminated(path: &str) -> String {
    let mut buf = String::new();
    buf.push_str(path);
    buf.push('\0');
    buf
}

fn print_entry_name(record: &[u8]) {
    let name_start = DIRENT64_HEADER_SIZE;
    let name_end = record[name_start..]
        .iter()
        .position(|&ch| ch == 0)
        .map(|pos| name_start + pos)
        .unwrap_or(record.len());
    if name_end == name_start {
        return;
    }

    let name = match str::from_utf8(&record[name_start..name_end]) {
        Ok(name) => name,
        Err(_) => return,
    };
    if name == "." || name == ".." {
        return;
    }
    print!("{}  ", name);
}

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    if argc > 2 {
        println!("usage: ls [dir]");
        return -1;
    }

    let default_path;
    let path = if argc == 2 {
        argv[1]
    } else {
        default_path = nul_terminated(".");
        default_path.as_str()
    };

    let fd = open(path, O_RDONLY | O_DIRECTORY, 0);
    if fd < 0 {
        println!(
            "ls: cannot open {}: {}",
            if argc == 2 { argv[1] } else { "." },
            fd
        );
        return fd as i32;
    }

    let fd = fd as usize;
    let mut buf = vec![0u8; BUF_SIZE];
    let size = getdents64(fd, buf.as_mut_slice());
    let close_ret = close(fd);
    if close_ret < 0 {
        return close_ret as i32;
    }
    if size < 0 {
        println!("ls: getdents64 failed: {}", size);
        return size as i32;
    }

    let size = size as usize;
    let mut offset = 0;
    while offset + DIRENT64_HEADER_SIZE <= size {
        let reclen = read_u16(&buf, offset + 16) as usize;
        if reclen < DIRENT64_HEADER_SIZE || offset + reclen > size {
            println!("ls: invalid dirent");
            return -1;
        }
        print_entry_name(&buf[offset..offset + reclen]);
        offset += reclen;
    }
    println!("");
    0
}
