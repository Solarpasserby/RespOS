#![no_std]
#![no_main]

extern crate user_lib;

#[unsafe(no_mangle)]
fn main(_argc: usize, _argv: &[&str]) -> i32 {
    let pid = user_lib::getpid();
    let tid = user_lib::gettid();
    if pid <= 0 || pid != tid {
        return 24;
    }
    23
}
