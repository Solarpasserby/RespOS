// user/src/bin/cat.rs

#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{O_RDONLY, close, open, read, write};

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    if argc != 2 {
        println!("usage: cat <file>");
        return -1;
    }

    let fd = open(argv[1], O_RDONLY, 0);
    if fd < 0 {
        println!("cat: cannot open {}: {}", argv[1], fd);
        return fd as i32;
    }

    let fd: usize = fd as usize;
    let mut buf = [0u8; 512];
    loop {
        let size = read(fd, &mut buf);
        if size < 0 {
            println!("cat: read error: {}", size);
            close(fd);
            return size as i32;
        }
        if size == 0 {
            break;
        }
        let mut written = 0;
        let size = size as usize;
        while written < size {
            let ret = write(1, &buf[written..size]);
            if ret < 0 {
                close(fd);
                return ret as i32;
            }
            written += ret as usize;
        }
    }

    let ret = close(fd);
    if ret < 0 {
        return ret as i32;
    }
    0
}
