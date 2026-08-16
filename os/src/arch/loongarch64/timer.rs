// os/src/arch/loongarch64/timer.rs
//
// LoongArch 系统定时器模块
// 使用 rdtime.d 指令读取稳定计数器，替代 RISC-V 的 mtime CSR。

use super::{register, sbi::set_timer};
use crate::config::{ACCOUNTING_CLOCK_FREQ, HARDWARE_CLOCK_FREQ, USER_CLOCK_FREQ};
use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

const TICKS_PER_SEC: usize = 100;
const MSEC_PER_SEC: usize = 1000;
const USEC_PER_SEC: usize = 1_000_000;
const TASK_TIMER_ADVANCE_US: usize = 800;
static REQUESTED_TASK_TIMER: AtomicUsize = AtomicUsize::new(usize::MAX);
static PROGRAMMED_SERVICE_TIMER: AtomicUsize = AtomicUsize::new(usize::MAX);

const LS7A_RTC_BASE: usize = 0x100d_0100;
const LS7A_TOY_READ0: usize = 0x2c;
const LS7A_TOY_READ1: usize = 0x30;
const LS7A_TOY_WRITE0: usize = 0x24;
const LS7A_TOY_WRITE1: usize = 0x28;
const LS7A_RTC_CTRL: usize = 0x40;
const LS7A_RTC_CTRL_ENABLE_TOY: u32 = (1 << 11) | (1 << 8);

fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn civil_from_days(days: i64) -> Option<(i64, i64, i64)> {
    let shifted = days.checked_add(719_468)?;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = shifted_month + if shifted_month < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    Some((year, month, day))
}

/// Read QEMU virt's LS7A TOY clock as nanoseconds since the Unix epoch.
pub fn rtc_epoch_ns() -> Option<u64> {
    let base = crate::config::KERNEL_BASE.checked_add(LS7A_RTC_BASE)?;
    unsafe {
        core::ptr::write_volatile((base + LS7A_RTC_CTRL) as *mut u32, LS7A_RTC_CTRL_ENABLE_TOY);
    }
    for _ in 0..4 {
        let year_before =
            unsafe { core::ptr::read_volatile((base + LS7A_TOY_READ1) as *const u32) };
        let packed = unsafe { core::ptr::read_volatile((base + LS7A_TOY_READ0) as *const u32) };
        let year_after = unsafe { core::ptr::read_volatile((base + LS7A_TOY_READ1) as *const u32) };
        if year_before != year_after {
            continue;
        }
        let year = i64::from(year_after).checked_add(1900)?;
        let month = i64::from((packed >> 26) & 0x3f);
        let day = i64::from((packed >> 21) & 0x1f);
        let hour = i64::from((packed >> 16) & 0x1f);
        let minute = i64::from((packed >> 10) & 0x3f);
        let second = i64::from((packed >> 4) & 0x3f);
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        let days = days_from_civil(year, month, day)?;
        let seconds = days
            .checked_mul(86_400)?
            .checked_add(hour * 3_600 + minute * 60 + second)?;
        return u64::try_from(seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1_000_000_000));
    }
    None
}

/// Set QEMU virt's LS7A TOY clock at one-second precision.
pub fn rtc_set_epoch_ns(epoch_ns: u64) -> bool {
    let seconds = epoch_ns / 1_000_000_000;
    let Ok(days) = i64::try_from(seconds / 86_400) else {
        return false;
    };
    let rem = seconds % 86_400;
    let Some((year, month, day)) = civil_from_days(days) else {
        return false;
    };
    let Some(toy_year) = year
        .checked_sub(1900)
        .and_then(|year| u32::try_from(year).ok())
    else {
        return false;
    };
    let hour = rem / 3_600;
    let minute = rem % 3_600 / 60;
    let second = rem % 60;
    let pack = |month: u64, day: u64, hour: u64, minute: u64, second: u64| {
        ((month as u32) << 26)
            | ((day as u32) << 21)
            | ((hour as u32) << 16)
            | ((minute as u32) << 10)
            | ((second as u32) << 4)
    };
    let Some(base) = crate::config::KERNEL_BASE.checked_add(LS7A_RTC_BASE) else {
        return false;
    };
    unsafe {
        core::ptr::write_volatile((base + LS7A_RTC_CTRL) as *mut u32, LS7A_RTC_CTRL_ENABLE_TOY);
        // Move through January 1 so changing between leap and non-leap years
        // cannot make QEMU normalize an intermediate February 29 value.
        core::ptr::write_volatile((base + LS7A_TOY_WRITE0) as *mut u32, pack(1, 1, 0, 0, 0));
        core::ptr::write_volatile((base + LS7A_TOY_WRITE1) as *mut u32, toy_year);
        core::ptr::write_volatile(
            (base + LS7A_TOY_WRITE0) as *mut u32,
            pack(month as u64, day as u64, hour, minute, second),
        );
    }
    let target_ns = seconds.saturating_mul(1_000_000_000);
    rtc_epoch_ns().is_some_and(|actual| actual.abs_diff(target_ns) < 2_000_000_000)
}

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
        let _cpucfg_freq = base_freq * mul / div;
        info!(
            "[timer] CPUCFG freq: {} Hz, hardware clock freq: {} Hz, user clock freq: {} Hz, accounting clock freq: {} Hz",
            _cpucfg_freq,
            get_hardware_clock_freq(),
            get_user_clock_freq(),
            get_accounting_clock_freq()
        );
    } else {
        warn!(
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
    ticks.max(4)
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

pub fn reset_task_timer_requests() {
    debug_assert!(crate::arch::smp::is_timer_service_hart());
    REQUESTED_TASK_TIMER.store(usize::MAX, Ordering::Release);
}

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
                set_timer(trigger.max(get_time().saturating_add(4)));
                return;
            }
            Err(current) => programmed = current,
        }
    }
}

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
