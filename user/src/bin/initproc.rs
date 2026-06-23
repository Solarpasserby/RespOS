// user/src/bin/initproc.rs

#![no_std]
#![no_main]

extern crate user_lib;

use user_lib::{exec, fork, wait, yield_};

#[unsafe(no_mangle)]
fn main() -> i32 {
    if fork() == 0 {
        #[cfg(feature = "eval")]
        {
            let argv = ["testrunner\0".as_ptr(), core::ptr::null()];
            exec("testrunner\0", &argv);
            // println!("[initproc] failed to exec testrunner, falling back to shell");
        }

        let argv = ["user_shell\0".as_ptr(), core::ptr::null()];
        exec("user_shell\0", &argv);
    } else {
        loop {
            let mut exit_code: i32 = 0;
            let pid = wait(&mut exit_code);
            if pid < 0 {
                yield_();
                continue;
            }
            /*
            println!(
                "[initproc] Released a zombie process, pid={}, exit_code={}",
                pid, exit_code,
            );
            */
        }
    }
    0
}
