// os/src/console.rs
//!
//! 内核控制台输出模块。

use crate::sbi::console_putchar;
use core::fmt::{self, Arguments, Write};

struct Stdout;

/// 逐字符写入串口，同时用于用户态 `print!` 和内核日志。
impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            console_putchar(c as usize);
        }
        Ok(())
    }
}

/// 底层打印函数，所有输出宏最终都调用它。
pub fn print(args: Arguments) {
    Stdout.write_fmt(args).unwrap();
}

#[cfg(feature = "log")]
mod log_impl {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// 日志等级：数值越大越详细。
    #[derive(PartialEq, PartialOrd, Copy, Clone)]
    pub enum LogLevel {
        Error = 1,
        Warn,
        Info,
        Debug,
        Trace,
    }

    /// 编译期过滤：只显示 Info 及以上级别的日志。
    const LOG_LEVEL: LogLevel = LogLevel::Info;

    /// 全局自增序号，给每条日志分配唯一编号。
    static LOG_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 按颜色码输出一行带序号的日志。
    fn print_color(color: u8, seq: usize, args: Arguments) {
        super::print(format_args!("\x1b[{}m", color)); // 设置颜色
        super::print(format_args!("[log:{}] ", seq)); // 序号前缀
        super::print(args); // 日志正文
        super::print(format_args!("\x1b[0m\n")); // 重置颜色并换行
    }

    /// 内核日志入口。级别不够则直接返回。
    pub fn log(level: LogLevel, args: Arguments) {
        if level > LOG_LEVEL {
            return;
        }

        let seq = LOG_SEQ.fetch_add(1, Ordering::Relaxed);

        match level {
            LogLevel::Error => print_color(31, seq, args), // 红色
            LogLevel::Warn => print_color(93, seq, args),  // 黄色
            LogLevel::Info => print_color(34, seq, args),  // 蓝色
            LogLevel::Debug => print_color(32, seq, args), // 绿色
            LogLevel::Trace => print_color(90, seq, args), // 灰色
        }
    }
}

#[cfg(feature = "log")]
pub use log_impl::{LogLevel, log};

/// 标准输出宏，不换行。
#[macro_export]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!($fmt $(, $($arg)+)?));
    };
}

/// 标准输出宏，自动追加换行。
#[macro_export]
macro_rules! println {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?));
    };
}

// 以下日志宏各有两个版本：
// - feature = "log" → 调用 console::log 输出日志
// - 无 feature      → 展开为空块 {}，编译器完全消除

#[cfg(feature = "log")]
#[macro_export]
macro_rules! error {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Error,
            format_args!($fmt $(, $($arg)+)?)
        );
    };
}

#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! error {
    ($fmt: literal $(, $($arg: tt)+)?) => {};
}

#[cfg(feature = "log")]
#[macro_export]
macro_rules! warn {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Warn,
            format_args!($fmt $(, $($arg)+)?)
        );
    };
}

#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! warn {
    ($fmt: literal $(, $($arg: tt)+)?) => {};
}

#[cfg(feature = "log")]
#[macro_export]
macro_rules! info {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Info,
            format_args!($fmt $(, $($arg)+)?)
        );
    };
}

#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! info {
    ($fmt: literal $(, $($arg: tt)+)?) => {};
}

#[cfg(feature = "log")]
#[macro_export]
macro_rules! debug {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Debug,
            format_args!($fmt $(, $($arg)+)?)
        );
    };
}

#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! debug {
    ($fmt: literal $(, $($arg: tt)+)?) => {};
}

#[cfg(feature = "log")]
#[macro_export]
macro_rules! trace {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::log(
            $crate::console::LogLevel::Trace,
            format_args!($fmt $(, $($arg)+)?)
        );
    };
}

#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! trace {
    ($fmt: literal $(, $($arg: tt)+)?) => {};
}
