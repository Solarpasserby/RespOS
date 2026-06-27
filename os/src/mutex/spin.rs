use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

use super::{MutexOperations, ffi::SendWrapper};

pub struct MutexGuard<'a, T: ?Sized, S: MutexOperations> {
    mutex: &'a SpinMutex<T, S>,
    support_guard: S::GuardData,
    _not_send: PhantomData<*mut ()>,
}

// 自旋锁 guard 表示一个很短的不可睡眠临界区，不能被移动到其他任务中。
unsafe impl<'a, T: ?Sized + Sync, S: MutexOperations> Sync for MutexGuard<'a, T, S> {}

/// 自旋互斥锁。
///
/// 这类锁适合保护非常短的临界区：拿不到锁时当前 CPU 会忙等，不会让出执行权。
/// `L` 描述加锁前后的附加动作，例如普通自旋锁不做额外处理，关中断自旋锁会在
/// 临界区期间关闭本 CPU 中断。
pub struct SpinMutex<T: ?Sized, L: MutexOperations> {
    _marker: PhantomData<L>,
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send, S: MutexOperations> Sync for SpinMutex<T, S> {}
unsafe impl<T: ?Sized + Send, S: MutexOperations> Send for SpinMutex<T, S> {}

impl<T, S: MutexOperations> SpinMutex<T, S> {
    /// 创建一把新的自旋锁。
    pub const fn new(user_data: T) -> Self {
        SpinMutex {
            lock: AtomicBool::new(false),
            _marker: PhantomData,
            data: UnsafeCell::new(user_data),
        }
    }

    /// 等待锁从“看起来已被占用”变成“可能可获取”。
    ///
    /// 这里使用 `Relaxed` 读取只是为了减少总线上的交换操作；真正取得锁时仍然会使用
    /// `Acquire` 语义的 CAS。`spin_loop` 会向 CPU 表明当前正在自旋等待。
    #[inline(always)]
    fn wait_unlock(&self) {
        let mut try_count = 0usize;
        while self.lock.load(Ordering::Relaxed) {
            core::hint::spin_loop();
            try_count += 1;
            if try_count >= 0x1000000000 {
                panic!("Mutex: deadlock detected! try_count > {:#x}\n", try_count);
            }
        }
    }

    /// 获取锁并返回 guard。
    ///
    /// guard 存活期间锁保持占用；guard 被丢弃时自动释放锁并执行 `S::after_unlock`。
    /// 自旋锁临界区应尽量短，不能在持有 guard 时执行可能阻塞、调度或 `.await` 的操作。
    #[inline(always)]
    pub fn lock(&self) -> MutexGuard<T, S> {
        let support_guard = S::before_lock();
        loop {
            self.wait_unlock();
            if self
                .lock
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
        MutexGuard {
            mutex: self,
            support_guard,
            _not_send: PhantomData,
        }
    }

    /// 尝试获取锁一次，不自旋等待。
    ///
    /// 主要用于诊断和尽力而为的统计路径；失败时会恢复 `before_lock`
    /// 改变的中断状态。
    #[inline(always)]
    pub fn try_lock(&self) -> Option<MutexGuard<T, S>> {
        let mut support_guard = S::before_lock();
        if self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            S::after_unlock(&mut support_guard);
            return None;
        }
        Some(MutexGuard {
            mutex: self,
            support_guard,
            _not_send: PhantomData,
        })
    }

    /// 获取一把允许跨任务移动的 guard。
    ///
    /// # Safety
    ///
    /// 调用者必须保证 guard 不会因为跨 `.await`、跨任务迁移或长时间持有而造成死锁。
    /// 普通代码应使用 `lock`，只有在实现更高层同步原语时才考虑使用这个接口。
    pub unsafe fn sent_lock(&self) -> impl DerefMut<Target = T> + '_ {
        SendWrapper::new(self.lock())
    }
}

impl<'a, T: ?Sized, S: MutexOperations> Deref for MutexGuard<'a, T, S> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexOperations> DerefMut for MutexGuard<'a, T, S> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexOperations> Drop for MutexGuard<'a, T, S> {
    #[inline(always)]
    fn drop(&mut self) {
        self.mutex.lock.store(false, Ordering::Release);
        S::after_unlock(&mut self.support_guard);
    }
}
