// user/src/bin/cd.rs

#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::str;
use user_lib::{chdir, getcwd};

fn print_cwd(label: &str) {
    let mut cwd = [0u8; 128];
    let ret = getcwd(&mut cwd);
    if ret < 0 {
        println!("{}: getcwd failed with {}", label, ret);
        return;
    }
    let len = cwd.iter().position(|&ch| ch == 0).unwrap_or(cwd.len());
    match str::from_utf8(&cwd[..len]) {
        Ok(path) => println!("{}: {}", label, path),
        Err(_) => println!("{}: invalid utf8", label),
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    print_cwd("before chdir");
    let ret = chdir("/\0");
    println!("chdir(\"/\") = {}", ret);
    print_cwd("after chdir");
    0
}
