// os/src/timer.rs

//! ### 系统计时器模块

use riscv::register::time;
use crate::config::CLOCK_FREQ;
use super::sbi::set_timer;

const TICKS_PER_SEC: usize = 100; // 每秒触发时钟中断的次数
const MSEC_PER_SEC: usize = 1000; // 微秒

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TimeSpec {
    // 秒数
    pub sec: usize,
    // 纳秒数
    pub nsec: usize,
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

/// 设置下一次时钟中断触发器
pub fn set_next_ti_trigger() {
    set_timer(get_time() + CLOCK_FREQ / TICKS_PER_SEC);
}

/// 读取硬件运行时间(ms)
pub fn get_time_ms() -> usize {
    time::read() / (CLOCK_FREQ / MSEC_PER_SEC)
}