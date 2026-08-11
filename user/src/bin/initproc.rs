// user/src/bin/initproc.rs

#![no_std]
#![no_main]

extern crate user_lib;

use user_lib::{exec, fork, println, wait, yield_};

#[unsafe(no_mangle)]
fn main() -> i32 {
    if fork() == 0 {
        let argv = ["contest_launcher\0".as_ptr(), core::ptr::null()];
        let _ = exec("contest_launcher\0", &argv);

        // Preserve the preliminary-round startup path if the dispatcher is
        // unavailable for any reason.
        let argv = ["testrunner\0".as_ptr(), core::ptr::null()];
        let _ = exec("testrunner\0", &argv);
        println!("[initproc] testrunner unavailable, falling back to shell");
        let argv = ["user_shell\0".as_ptr(), core::ptr::null()];
        let _ = exec("user_shell\0", &argv);
    } else {
        loop {
            let mut exit_code: i32 = 0;
            let pid = wait(&mut exit_code);
            if pid < 0 {
                yield_();
            }
        }
    }
    0
}
