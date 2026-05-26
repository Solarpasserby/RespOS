#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{chdir, exec, exit, fork, poweroff, waitpid};

const BASIC_TESTS: &[&str] = &[
    "brk\0",
    "chdir\0",
    "clone\0",
    "close\0",
    "dup2\0",
    "dup\0",
    "execve\0",
    "exit\0",
    "fork\0",
    "fstat\0",
    "getcwd\0",
    "getdents\0",
    "getpid\0",
    "getppid\0",
    "gettimeofday\0",
    "mkdir_\0",
    "mmap\0",
    "mount\0",
    "munmap\0",
    "openat\0",
    "open\0",
    "pipe\0",
    "read\0",
    "sleep\0",
    "times\0",
    "umount\0",
    "uname\0",
    "unlink\0",
    "wait\0",
    "waitpid\0",
    "write\0",
    "yield\0",
];

fn strip_nul(s: &str) -> &str {
    &s[..s.len() - 1]
}

fn run_program(path: &str) -> i32 {
    println!("Testing {} :", strip_nul(path));

    let pid = fork();
    if pid == 0 {
        let argv = [path.as_ptr(), core::ptr::null()];
        let ret = exec(path, &argv);
        println!("[testrunner] exec {} failed: {}", strip_nul(path), ret);
        exit(-1);
        unreachable!();
    }

    if pid < 0 {
        println!("[testrunner] fork failed");
        return -1;
    }

    let mut exit_code = 0;
    let waited = waitpid(pid as usize, &mut exit_code);
    if waited < 0 {
        println!("[testrunner] wait failed: {}", waited);
        return -1;
    }
    exit_code
}

fn run_basic_musl() {
    println!("#### OS COMP TEST GROUP START basic-musl ####");
    if chdir("/musl/basic\0") < 0 {
        println!("[testrunner] skip basic-musl: cannot enter /musl/basic");
        println!("#### OS COMP TEST GROUP END basic-musl ####");
        return;
    }

    for test in BASIC_TESTS {
        let exit_code = run_program(test);
        if exit_code != 0 {
            println!(
                "[testrunner] {} exited with code {}",
                strip_nul(test),
                exit_code
            );
        }
    }
    let _ = chdir("/\0");
    println!("#### OS COMP TEST GROUP END basic-musl ####");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[testrunner] start");
    run_basic_musl();
    println!("[testrunner] all selected tests finished, powering off");
    poweroff();
    0
}
