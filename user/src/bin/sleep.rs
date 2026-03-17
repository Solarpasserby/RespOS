#![no_std]
#![no_main]

use user_lib::{time_get, yield_};

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let current_timer = time_get();
    let wait_time = current_timer + 3000;
    while time_get() < wait_time {
        yield_();
    }
    println!("main03: I'm awake now!");
    0
}
