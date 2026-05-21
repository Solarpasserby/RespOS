use core::ops::{Deref, DerefMut};

use super::MutexOperations;
use crate::arch::interrupt::InterruptGuard;

/// 不做任何额外处理的锁策略。
///
/// 适用于不会被中断上下文重入访问的数据。它只依赖锁本身提供互斥，
/// 加锁前后不会改变当前 CPU 的中断状态。
pub struct NoopLock;

impl MutexOperations for NoopLock {
    type GuardData = ();
    #[inline(always)]
    fn before_lock() -> Self::GuardData {}
    #[inline(always)]
    fn after_unlock(_: &mut Self::GuardData) {}
}

/// 关中断锁策略。
///
/// 加锁前关闭本 CPU 中断，解锁后由 `InterruptGuard` 的析构逻辑恢复之前的中断状态。
/// 这种策略用于保护可能被中断处理函数访问的数据，避免“普通内核代码持锁时被中断打断，
/// 中断处理函数再次尝试获取同一把锁”导致的单核死锁。
pub struct NoIrqLock;

impl MutexOperations for NoIrqLock {
    type GuardData = InterruptGuard;
    #[inline(always)]
    fn before_lock() -> Self::GuardData {
        InterruptGuard::new()
    }
    #[inline(always)]
    fn after_unlock(_: &mut Self::GuardData) {}
}

/// 显式把内部值标记为可跨任务发送的包装器。
///
/// `SpinMutexGuard` 默认不应该跨 `await` 或任务边界移动，因为这会让一个短临界区
/// 被异步挂起点拉长，甚至造成死锁。少数底层实现确实需要这样做时，必须通过
/// `unsafe` 接口主动包一层 `SendWrapper`，把风险暴露给调用者。
pub struct SendWrapper<T>(pub T);

impl<T> SendWrapper<T> {
    #[inline(always)]
    pub fn new(data: T) -> Self {
        SendWrapper(data)
    }
}

unsafe impl<T> Send for SendWrapper<T> {}
unsafe impl<T> Sync for SendWrapper<T> {}

impl<T: Deref> Deref for SendWrapper<T> {
    type Target = T::Target;
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<T: DerefMut> DerefMut for SendWrapper<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}
