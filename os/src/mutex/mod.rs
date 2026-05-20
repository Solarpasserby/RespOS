mod ffi;
mod sleep;
mod spin;

use alloc::sync::Arc;
pub use ffi::{NoIrqLock, NoopLock};
pub use spin::MutexGuard;

pub type SpinLock<T> = spin::SpinMutex<T, ffi::NoopLock>;
pub type SpinNoIrqLock<T> = spin::SpinMutex<T, ffi::NoIrqLock>;
pub type SleepLock<T> = sleep::SleepMutex<T, ffi::NoIrqLock>;
pub type Shared<T> = Arc<SpinNoIrqLock<T>>;
pub type SleepShared<T> = Arc<SleepLock<T>>;

/// 锁的附加操作策略。
///
/// `SpinMutex` 和 `SleepMutex` 只负责互斥本身；是否需要在加锁前关闭中断、
/// 是否需要在解锁后恢复中断状态，则交给这个 trait 来描述。这样同一份锁实现
/// 可以组合出普通自旋锁、关中断自旋锁、关中断睡眠锁等不同语义。
pub trait MutexOperations {
    /// 加锁前保存的上下文数据。
    ///
    /// 例如 `NoIrqLock` 会在加锁前关闭本 CPU 中断，并把旧的中断状态保存在
    /// `InterruptGuard` 中；普通锁不需要额外状态，因此使用 `()`。
    type GuardData;

    /// 在尝试获取锁之前调用。
    ///
    /// 这个函数的返回值会保存在 guard 中，直到 guard 被释放。对于关中断锁来说，
    /// 这保证了整个临界区都处于“不会被本 CPU 中断处理函数重入”的状态。
    fn before_lock() -> Self::GuardData;

    /// 在 guard 释放锁之后调用，用于恢复 `before_lock` 中改变的状态。
    fn after_unlock(_: &mut Self::GuardData);
}

pub fn new_shared<T>(data: T) -> Shared<T> {
    Arc::new(SpinNoIrqLock::new(data))
}

pub fn new_sleep_shared<T>(data: T) -> SleepShared<T> {
    Arc::new(SleepLock::new(data))
}
