// os/src/arch/loongarch64/timer.rs
//
// LoongArch 系统定时器模块
// 使用 rdtime.d 指令读取稳定计数器，替代 RISC-V 的 mtime CSR。

use super::sbi::set_timer;
use crate::config::CLOCK_FREQ;
use core::arch::asm;

const TICKS_PER_SEC: usize = 100;
const MSEC_PER_SEC: usize = 1000;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TimeSpec {
    /// 秒数
    pub sec: usize,
    /// 纳秒数
    pub nsec: usize,
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct StatxTimeStamp {
    /// 自 UNIX 时间以来的秒数
    pub sec: i64,
    /// 纳秒数
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

/// 读取 LoongArch 稳定计数器的值（rdtime.d）
///
/// rdtime.d 将 64 位计数器拆分为低 32 位和高 32 位分别写入两个寄存器
pub fn get_time() -> usize {
    let low: usize;
    let high: usize;
    unsafe {
        asm!(
            "rdtime.d {}, {}",
            out(reg) low,
            out(reg) high,
            options(nomem, nostack)
        );
    }
    (high << 32) | (low & 0xFFFFFFFF)
}

/// 设置下一次时钟中断触发
pub fn set_next_ti_trigger() {
    set_timer(get_time() + CLOCK_FREQ / TICKS_PER_SEC);
}

/// 读取硬件运行时间（毫秒）
pub fn get_time_ms() -> usize {
    get_time() / (CLOCK_FREQ / MSEC_PER_SEC)
}
