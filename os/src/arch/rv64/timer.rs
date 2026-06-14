// os/src/timer.rs

//! ### 系统计时器模块

use super::sbi::set_timer;
use crate::config::{ACCOUNTING_CLOCK_FREQ, HARDWARE_CLOCK_FREQ, USER_CLOCK_FREQ};
use riscv::register::time;

const TICKS_PER_SEC: usize = 100; // 每秒触发时钟中断的次数
const MSEC_PER_SEC: usize = 1000; // 微秒
const USEC_PER_SEC: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TimeSpec {
    // 秒数
    pub sec: isize,
    // 纳秒数
    pub nsec: isize,
}

impl TimeSpec {
    pub fn is_valid_duration(&self) -> bool {
        self.sec >= 0 && self.nsec >= 0 && self.nsec < 1_000_000_000
    }

    pub fn is_zero(&self) -> bool {
        self.sec == 0 && self.nsec == 0
    }

    pub fn checked_duration_ms(&self) -> Option<usize> {
        if !self.is_valid_duration() {
            return None;
        }
        (self.sec as usize)
            .checked_mul(1000)
            .and_then(|ms| ms.checked_add((self.nsec as usize).div_ceil(1_000_000)))
    }

    pub fn checked_duration_us(&self) -> Option<usize> {
        if !self.is_valid_duration() {
            return None;
        }
        (self.sec as usize)
            .checked_mul(1_000_000)
            .and_then(|us| us.checked_add((self.nsec as usize).div_ceil(1000)))
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct StatxTimeStamp {
    /// 自 UNIX time以来的秒数
    pub sec: i64,
    /// 纳秒数, 秒数后剩余小数部分
    pub nsec: u32,
}

impl StatxTimeStamp {
    pub fn new() -> Self {
        let current_time = get_time_ms();
        Self {
            sec: (current_time / 1000) as i64,
            nsec: ((current_time % 1000) * 1000000) as u32,
        }
    }
}

impl From<TimeSpec> for StatxTimeStamp {
    fn from(ts: TimeSpec) -> Self {
        Self {
            sec: ts.sec as i64,
            nsec: ts.nsec as u32,
        }
    }
}

/// 获取 `mtime` 的值
pub fn get_time() -> usize {
    time::read()
}

#[inline(always)]
fn ticks_to_ms(ticks: usize, freq: usize) -> usize {
    ticks / (freq / MSEC_PER_SEC)
}

#[inline(always)]
fn ticks_to_us(ticks: usize, freq: usize) -> usize {
    ticks / freq * USEC_PER_SEC + ticks % freq * USEC_PER_SEC / freq
}

#[inline(always)]
pub fn get_hardware_clock_freq() -> usize {
    HARDWARE_CLOCK_FREQ
}

#[inline(always)]
pub fn get_user_clock_freq() -> usize {
    USER_CLOCK_FREQ
}

#[inline(always)]
pub fn get_accounting_clock_freq() -> usize {
    ACCOUNTING_CLOCK_FREQ
}

/// 设置下一次时钟中断触发器
pub fn set_next_ti_trigger() {
    set_timer(get_time() + get_hardware_clock_freq() / TICKS_PER_SEC);
}

/// 读取用户可见运行时间(ms)
pub fn get_time_ms() -> usize {
    ticks_to_ms(get_time(), get_user_clock_freq())
}

/// 读取用户可见运行时间(us)
pub fn get_time_us() -> usize {
    ticks_to_us(get_time(), get_user_clock_freq())
}

/// RISC-V 目前 timeout 使用同一套硬件尺度。
pub fn get_timeout_ms() -> usize {
    ticks_to_ms(get_time(), get_hardware_clock_freq())
}

/// RISC-V 目前 timeout 使用同一套硬件尺度。
pub fn get_timeout_us() -> usize {
    ticks_to_us(get_time(), get_hardware_clock_freq())
}

/// 读取 CPU 时间记账使用的运行时间(ms)。
pub fn get_accounting_ms() -> usize {
    ticks_to_ms(get_time(), get_accounting_clock_freq())
}

/// 读取 CPU 时间记账使用的运行时间(us)。
pub fn get_accounting_us() -> usize {
    ticks_to_us(get_time(), get_accounting_clock_freq())
}
