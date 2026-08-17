// os/src/timer.rs

//! ### 系统计时器模块

use super::sbi::set_timer;
use crate::config::{ACCOUNTING_CLOCK_FREQ, HARDWARE_CLOCK_FREQ, USER_CLOCK_FREQ};
use core::sync::atomic::{AtomicUsize, Ordering};
use riscv::register::time;

const TICKS_PER_SEC: usize = 100; // 每秒触发时钟中断的次数
const MSEC_PER_SEC: usize = 1000; // 微秒
const USEC_PER_SEC: usize = 1_000_000;
const TASK_TIMER_ADVANCE_US: usize = 800;
static REQUESTED_TASK_TIMER: AtomicUsize = AtomicUsize::new(usize::MAX);
static PROGRAMMED_SERVICE_TIMER: AtomicUsize = AtomicUsize::new(usize::MAX);

const GOLDFISH_RTC_BASE: usize = 0x0010_1000;
const GOLDFISH_RTC_TIME_LOW: usize = 0x00;
const GOLDFISH_RTC_TIME_HIGH: usize = 0x04;

/// Read QEMU virt's goldfish RTC as nanoseconds since the Unix epoch.
pub fn rtc_epoch_ns() -> Option<u64> {
    let base = crate::config::KERNEL_BASE.checked_add(GOLDFISH_RTC_BASE)?;
    for _ in 0..4 {
        let high_before =
            unsafe { core::ptr::read_volatile((base + GOLDFISH_RTC_TIME_HIGH) as *const u32) };
        let low = unsafe { core::ptr::read_volatile((base + GOLDFISH_RTC_TIME_LOW) as *const u32) };
        let high_after =
            unsafe { core::ptr::read_volatile((base + GOLDFISH_RTC_TIME_HIGH) as *const u32) };
        if high_before == high_after {
            return Some(((high_after as u64) << 32) | low as u64);
        }
    }
    None
}

/// Set QEMU virt's goldfish RTC.  Goldfish follows the same high-then-low
/// programming order used by Linux's rtc-goldfish driver.
pub fn rtc_set_epoch_ns(epoch_ns: u64) -> bool {
    let Some(base) = crate::config::KERNEL_BASE.checked_add(GOLDFISH_RTC_BASE) else {
        return false;
    };
    unsafe {
        core::ptr::write_volatile(
            (base + GOLDFISH_RTC_TIME_HIGH) as *mut u32,
            (epoch_ns >> 32) as u32,
        );
        core::ptr::write_volatile((base + GOLDFISH_RTC_TIME_LOW) as *mut u32, epoch_ns as u32);
    }
    rtc_epoch_ns().is_some_and(|actual| actual.abs_diff(epoch_ns) < 1_000_000_000)
}

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
    let deadline = get_time() + get_hardware_clock_freq() / TICKS_PER_SEC;
    if crate::arch::smp::is_timer_service_hart() {
        PROGRAMMED_SERVICE_TIMER.store(deadline, Ordering::Release);
    }
    set_timer(deadline);
}

#[inline]
fn hardware_ticks_from_us_ceil(us: usize) -> usize {
    let freq = get_hardware_clock_freq();
    let ticks = (us / USEC_PER_SEC).saturating_mul(freq).saturating_add(
        (us % USEC_PER_SEC)
            .saturating_mul(freq)
            .div_ceil(USEC_PER_SEC),
    );
    ticks.max(1)
}

/// Publish a task timeout as a reason to shorten the timer-service hart's
/// normal scheduler tick. Stale requests only cause an extra early interrupt;
/// every safe high-level timer scan rebuilds the minimum from live waiters.
pub fn request_task_timer_after_us(delay_us: usize) {
    let deadline = get_time().saturating_add(hardware_ticks_from_us_ceil(delay_us));
    let previous = REQUESTED_TASK_TIMER.fetch_min(deadline, Ordering::AcqRel);
    if deadline >= previous {
        return;
    }
    if crate::arch::smp::is_timer_service_hart() {
        rearm_task_timer_request();
    } else {
        crate::arch::smp::kick_timer_service_hart();
    }
}

/// Begin rebuilding the exact-deadline hint from the authoritative timeout
/// registries. Only the timer-service hart performs the high-level scan.
pub fn reset_task_timer_requests() {
    debug_assert!(crate::arch::smp::is_timer_service_hart());
    REQUESTED_TASK_TIMER.store(usize::MAX, Ordering::Release);
}

/// Shorten the currently programmed service timer when a newly published task
/// deadline precedes the 100 Hz scheduler tick.
pub fn rearm_task_timer_request() {
    if !crate::arch::smp::is_timer_service_hart() {
        return;
    }
    let requested = REQUESTED_TASK_TIMER.load(Ordering::Acquire);
    let trigger = requested.saturating_sub(hardware_ticks_from_us_ceil(TASK_TIMER_ADVANCE_US));
    let mut programmed = PROGRAMMED_SERVICE_TIMER.load(Ordering::Acquire);
    while trigger < programmed {
        match PROGRAMMED_SERVICE_TIMER.compare_exchange_weak(
            programmed,
            trigger,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                set_timer(trigger.max(get_time().saturating_add(1)));
                return;
            }
            Err(current) => programmed = current,
        }
    }
}

/// QEMU may inject a one-shot timer several hundred microseconds after its
/// programmed compare value. Arm slightly early, then wait only inside this
/// bounded window so the authoritative software deadline is never observed
/// early and a second emulated timer injection is avoided.
pub fn await_task_timer_deadline() {
    if !crate::arch::smp::is_timer_service_hart() {
        return;
    }
    let advance = hardware_ticks_from_us_ceil(TASK_TIMER_ADVANCE_US);
    loop {
        let requested = REQUESTED_TASK_TIMER.load(Ordering::Acquire);
        if requested == usize::MAX {
            return;
        }
        let now = get_time();
        if now >= requested || requested - now > advance {
            return;
        }
        core::hint::spin_loop();
    }
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
