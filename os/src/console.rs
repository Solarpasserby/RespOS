// os/src/console.rs

// 目前主要用于~~终端~~DEBUG输出

use crate::sbi::console_putchar;
use core::fmt::{Write, Result, Arguments};
struct Stdout;

impl Write for Stdout {
    // Using an iter to reduce code 
    fn write_str(&mut self, s: &str) -> Result {
        s.chars().for_each(|c| console_putchar(c as usize));
        Ok(())
    }
}

pub fn print(args: Arguments) {
    Stdout.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!($fmt $(, $($arg)+)?));
    }
}

#[macro_export]
macro_rules! println {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?));
    }
}