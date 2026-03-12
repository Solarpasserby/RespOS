#![no_std]
#![no_main]

use user_lib::yield_;

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("main03: simple test");
    yield_();
    println!("main03: 1DSAJDJASDSAD!");
    yield_();
    println!("main03: 1DSAJDJASDSAD!");
    0
}
