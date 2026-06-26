#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::string::String;
use user_lib::{
    O_CREATE, O_RDONLY, O_TRUNC, O_WRONLY, Stat, close, copy_file_range, fstat, open, read, unlink,
    write,
};

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn join_dest(src: &str, dst: &str) -> String {
    if dst == "." {
        return String::from(basename(src));
    }
    if dst.ends_with('/') {
        let mut out = String::from(dst);
        out.push_str(basename(src));
        return out;
    }
    String::from(dst)
}

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    if argc != 3 {
        println!("usage: cp <source> <dest>");
        return 1;
    }

    let src_fd = open(argv[1], O_RDONLY, 0);
    if src_fd < 0 {
        println!("cp: cannot open {}: {}", argv[1], src_fd);
        return 1;
    }

    let dst_path = join_dest(argv[1], argv[2]);
    let mut dst_sys_path = dst_path.clone();
    dst_sys_path.push('\0');
    let _ = unlink(dst_sys_path.as_str());
    let dst_fd = open(dst_sys_path.as_str(), O_WRONLY | O_CREATE | O_TRUNC, 0o755);
    if dst_fd < 0 {
        println!("cp: cannot create {}: {}", dst_path, dst_fd);
        let _ = close(src_fd as usize);
        return 1;
    }

    let src_fd = src_fd as usize;
    let dst_fd = dst_fd as usize;
    let mut stat = Stat::default();
    if fstat(src_fd, &mut stat) == 0 {
        let mut copied = 0usize;
        let total = stat.st_size as usize;
        while copied < total {
            let ret = copy_file_range(src_fd, dst_fd, total - copied);
            if ret <= 0 {
                break;
            }
            copied += ret as usize;
        }
        if copied == total {
            let src_ret = close(src_fd);
            let dst_ret = close(dst_fd);
            return if src_ret < 0 || dst_ret < 0 { 1 } else { 0 };
        }
    }

    let mut buf = [0u8; 32 * 1024];
    loop {
        let n = read(src_fd, &mut buf);
        if n < 0 {
            println!("cp: read error: {}", n);
            let _ = close(src_fd);
            let _ = close(dst_fd);
            return 1;
        }
        if n == 0 {
            break;
        }

        let mut written = 0usize;
        let n = n as usize;
        while written < n {
            let ret = write(dst_fd, &buf[written..n]);
            if ret < 0 {
                println!("cp: write error: {}", ret);
                let _ = close(src_fd);
                let _ = close(dst_fd);
                return 1;
            }
            if ret == 0 {
                println!("cp: short write");
                let _ = close(src_fd);
                let _ = close(dst_fd);
                return 1;
            }
            written += ret as usize;
        }
    }

    let src_ret = close(src_fd);
    let dst_ret = close(dst_fd);
    if src_ret < 0 || dst_ret < 0 {
        return 1;
    }
    0
}
