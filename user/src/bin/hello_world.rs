#![no_std]
#![no_main]

use user_lib::yield_;

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("main01: Hello, world!");
    yield_();
    println!("main01: THAT'S WHAT HAPPENED AFTER YIELDING!");
    yield_();
    // panic!("test tracing.");
    println!("main01: Try again!");
    0
}
