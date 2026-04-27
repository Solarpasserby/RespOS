use crate::sbi::console_putchar;
use core::fmt::{self, Write, Arguments};

/// ===== 日志等级 =====
#[derive(PartialEq, PartialOrd, Copy, Clone)]
pub enum LogLevel {
    Error = 1,
    Warn,
    Info,
    Debug,
    Trace,
}

/// ===== 当前日志等级（可以调） =====
/// 以后可以做成从 Makefile 传参
const LOG_LEVEL: LogLevel = LogLevel::Info;

/// ===== 输出结构 =====
struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            console_putchar(c as usize);
        }
        Ok(())
    }
}

/// ===== 基础打印 =====
pub fn print(args: Arguments) {
    Stdout.write_fmt(args).unwrap();
}

/// ===== 彩色打印核心 =====
fn print_color(color: u8, args: Arguments) {
    // 开始颜色
    print(format_args!("\x1b[{}m", color));
    // 内容
    print(args);
    // 结束颜色
    print(format_args!("\x1b[0m\n"));
}

/// ===== 日志打印（带等级控制） =====
pub fn log(level: LogLevel, args: Arguments) {
    if level > LOG_LEVEL {
        return;
    }

    match level {
        LogLevel::Error => print_color(31, args), // 红
        LogLevel::Warn  => print_color(93, args), // 黄
        LogLevel::Info  => print_color(34, args), // 蓝
        LogLevel::Debug => print_color(32, args), // 绿
        LogLevel::Trace => print_color(90, args), // 灰
    }
}

/// ===== 宏系统 =====

#[macro_export]
macro_rules! error {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Error,
            format_args!($fmt $(, $($arg)+)?)
        );
    }
}

#[macro_export]
macro_rules! warn {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Warn,
            format_args!($fmt $(, $($arg)+)?)
        );
    }
}

#[macro_export]
macro_rules! info {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Info,
            format_args!($fmt $(, $($arg)+)?)
        );
    }
}

#[macro_export]
macro_rules! debug {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Debug,
            format_args!($fmt $(, $($arg)+)?)
        );
    }
}

#[macro_export]
macro_rules! trace {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Trace,
            format_args!($fmt $(, $($arg)+)?)
        );
    }
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