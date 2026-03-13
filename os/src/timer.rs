// os/src/timer.rs

//! ### 系统计时器模块

use riscv::register::time;
use crate::config::CLOCK_FREQ;
use crate::sbi::set_timer;

const TICKS_PER_SEC: usize = 100; // 每秒触发时钟中断的次数
const MSEC_PER_SEC: usize = 1000; // 微秒

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