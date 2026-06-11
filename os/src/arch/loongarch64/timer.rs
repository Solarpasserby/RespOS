// os/src/arch/loongarch64/timer.rs
//
// LoongArch 系统定时器模块
// 使用 rdtime.d 指令读取稳定计数器，替代 RISC-V 的 mtime CSR。

use super::{register, sbi::set_timer};
use crate::config::DEFAULT_CLOCK_FREQ;
use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

const TICKS_PER_SEC: usize = 100;
const MSEC_PER_SEC: usize = 1000;
const USEC_PER_SEC: usize = 1_000_000;

// TODO: 动态读取计算机运行频率，目前关闭
static CLOCK_FREQ_HZ: AtomicUsize = AtomicUsize::new(DEFAULT_CLOCK_FREQ);
const USE_CPUCFG_CLOCK_FREQ: bool = false;

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
pub fn get_clock_freq() -> usize {
    // CLOCK_FREQ_HZ.load(Ordering::Relaxed)
    DEFAULT_CLOCK_FREQ
}

/// Read the stable counter frequency from CPUCFG when the platform exposes it.
pub fn init_clock_freq() {
    let base_freq = cpucfg(4) & 0xffff_ffff;
    let cfg5 = cpucfg(5);
    let mul = cfg5 & 0xffff;
    let div = (cfg5 >> 16) & 0xffff;

    if base_freq != 0 && mul != 0 && div != 0 {
        let cpucfg_freq = base_freq * mul / div;
        if USE_CPUCFG_CLOCK_FREQ {
            CLOCK_FREQ_HZ.store(cpucfg_freq, Ordering::Relaxed);
        }
        println!(
            "[timer] CPUCFG freq: {} Hz, active clock freq: {} Hz",
            cpucfg_freq,
            get_clock_freq()
        );
    } else {
        println!(
            "[timer] invalid CPUCFG timer freq base={} mul={} div={}, use default {} Hz",
            base_freq, mul, div, DEFAULT_CLOCK_FREQ
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
    set_timer(get_time() + get_clock_freq() / TICKS_PER_SEC);
}

/// 读取硬件运行时间（毫秒）
pub fn get_time_ms() -> usize {
    get_time() / (get_clock_freq() / MSEC_PER_SEC)
}

/// 读取硬件运行时间（微秒）
pub fn get_time_us() -> usize {
    let ticks = get_time();
    let clock_freq = get_clock_freq();
    ticks / clock_freq * USEC_PER_SEC + ticks % clock_freq * USEC_PER_SEC / clock_freq
}
