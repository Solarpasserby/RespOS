#![no_std]
#![no_main]

extern crate user_lib;

#[unsafe(no_mangle)]
pub fn main(_argc: usize, _argv: &[&str]) -> i32 {
    0
}
