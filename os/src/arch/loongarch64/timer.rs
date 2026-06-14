// os/src/arch/loongarch64/timer.rs
//
// LoongArch 系统定时器模块
// 使用 rdtime.d 指令读取稳定计数器，替代 RISC-V 的 mtime CSR。

use super::{register, sbi::set_timer};
use crate::config::{ACCOUNTING_CLOCK_FREQ, HARDWARE_CLOCK_FREQ, USER_CLOCK_FREQ};
use core::arch::asm;

const TICKS_PER_SEC: usize = 100;
const MSEC_PER_SEC: usize = 1000;
const USEC_PER_SEC: usize = 1_000_000;

// 时间频率刻意分成三类：
// - hardware clock: timer interrupt 和 timeout 使用真实硬件尺度；
// - user clock: gettimeofday/clock_gettime 使用，可为 bench 调整；
// - accounting clock: times()/getrusage() 等 CPU 时间记账使用。

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TimeSpec {
    /// 秒数
    pub sec: isize,
    /// 纳秒数
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

#[inline(always)]
fn cpucfg(index: usize) -> usize {
    let bits: usize;
    unsafe {
        asm!("cpucfg {0}, {1}", out(reg) bits, in(reg) index, options(nomem, nostack));
    }
    bits
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

/// Read CPUCFG only as a boot-time diagnostic. Runtime clock policy comes from board.rs.
pub fn init_clock_freq() {
    let base_freq = cpucfg(4) & 0xffff_ffff;
    let cfg5 = cpucfg(5);
    let mul = cfg5 & 0xffff;
    let div = (cfg5 >> 16) & 0xffff;

    if base_freq != 0 && mul != 0 && div != 0 {
        let cpucfg_freq = base_freq * mul / div;
        println!(
            "[timer] CPUCFG freq: {} Hz, hardware clock freq: {} Hz, user clock freq: {} Hz, accounting clock freq: {} Hz",
            cpucfg_freq,
            get_hardware_clock_freq(),
            get_user_clock_freq(),
            get_accounting_clock_freq()
        );
    } else {
        println!(
            "[timer] invalid CPUCFG timer freq base={} mul={} div={}, hardware clock freq: {} Hz, user clock freq: {} Hz, accounting clock freq: {} Hz",
            base_freq,
            mul,
            div,
            get_hardware_clock_freq(),
            get_user_clock_freq(),
            get_accounting_clock_freq()
        );
    }
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
pub fn get_time() -> usize {
    register::timer::read_time()
}

/// 设置下一次时钟中断触发
pub fn set_next_ti_trigger() {
    set_timer(get_time() + get_hardware_clock_freq() / TICKS_PER_SEC);
}

/// 读取用户可见运行时间（毫秒）
pub fn get_time_ms() -> usize {
    ticks_to_ms(get_time(), get_user_clock_freq())
}

/// 读取用户可见运行时间（微秒）
pub fn get_time_us() -> usize {
    ticks_to_us(get_time(), get_user_clock_freq())
}

/// 读取 timeout/deadline 使用的真实运行时间（毫秒）。
pub fn get_timeout_ms() -> usize {
    ticks_to_ms(get_time(), get_hardware_clock_freq())
}

/// 读取 timeout/deadline 使用的真实运行时间（微秒）。
pub fn get_timeout_us() -> usize {
    ticks_to_us(get_time(), get_hardware_clock_freq())
}

/// 读取 CPU 时间记账使用的运行时间（毫秒）。
pub fn get_accounting_ms() -> usize {
    ticks_to_ms(get_time(), get_accounting_clock_freq())
}

/// 读取 CPU 时间记账使用的运行时间（微秒）。
pub fn get_accounting_us() -> usize {
    ticks_to_us(get_time(), get_accounting_clock_freq())
}
