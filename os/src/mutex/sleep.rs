use super::{MutexOperations, spin::SpinMutex};
use alloc::{collections::VecDeque, sync::Arc};
use core::{
    cell::UnsafeCell,
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};

/// 睡眠互斥锁。
///
/// 和自旋锁不同，睡眠锁在拿不到锁时不会一直忙等，而是把当前 future 的 waker
/// 放入等待队列并返回 `Poll::Pending`。释放锁时会把所有权交给队首等待者并唤醒它。
///
/// 这类锁适合保护可能较长的临界区。它不能在中断上下文中使用，因为中断处理函数
/// 不能睡眠，也不能依赖异步调度器再次唤醒。
pub struct SleepMutex<T: ?Sized, S: MutexOperations> {
    /// 保护睡眠锁自身状态的短自旋锁。
    ///
    /// 等待队列和 `is_locked` 只在持有这把内部锁时访问，内部锁的临界区必须非常短。
    lock: SpinMutex<MutexInner, S>,
    /// 被保护的实际数据。
    data: UnsafeCell<T>,
}

struct MutexInner {
    is_locked: bool,
    wait_queue: VecDeque<Arc<GrantInfo>>,
}

/// 释放当前持有者对睡眠锁的所有权。
///
/// 如果等待队列非空，锁不会变为空闲，而是直接把所有权转交给队首等待者；
/// 如果没有等待者，才把 `is_locked` 清为 false。
fn wake_next_or_unlock(inner: &mut MutexInner) {
    if let Some(waiter) = inner.wait_queue.pop_front() {
        // 锁的所有权直接交给队首等待者，因此这里不把 `is_locked` 置回 false。
        let waker = unsafe { &mut *waiter.waker.get() }.take();
        waiter.is_granted.store(true, Ordering::Release);
        if let Some(waker) = waker {
            waker.wake();
        }
    } else {
        // 没有等待者时，锁才真正回到空闲状态。
        inner.is_locked = false;
    }
}

unsafe impl<T: ?Sized + Send, S: MutexOperations> Send for SleepMutex<T, S> {}
unsafe impl<T: ?Sized + Send, S: MutexOperations> Sync for SleepMutex<T, S> {}

impl<T, S: MutexOperations> SleepMutex<T, S> {
    /// 新建一个睡眠锁
    pub const fn new(user_data: T) -> Self {
        SleepMutex {
            lock: SpinMutex::new(MutexInner {
                is_locked: false,
                wait_queue: VecDeque::new(),
            }),
            data: UnsafeCell::new(user_data),
        }
    }
}

impl<T: ?Sized + Send, S: MutexOperations> SleepMutex<T, S> {
    /// 异步获取锁。
    ///
    /// 如果锁空闲，当前任务会立刻获得 guard；如果锁已被占用，当前任务会挂入 FIFO
    /// 等待队列，并在前一个 guard 释放时被唤醒。
    #[inline]
    pub async fn lock(&self) -> impl DerefMut<Target = T> + Send + '_ {
        SleepMutexFuture::new(self).await
    }
}

struct GrantInfo {
    is_granted: AtomicBool,
    waker: UnsafeCell<Option<Waker>>,
}

unsafe impl Send for GrantInfo {}
unsafe impl Sync for GrantInfo {}

struct SleepMutexFuture<'a, T: ?Sized, S: MutexOperations> {
    mutex: &'a SleepMutex<T, S>,
    grant: Arc<GrantInfo>,
    queued: bool,
    completed: bool,
}

impl<'a, T: ?Sized, S: MutexOperations> SleepMutexFuture<'a, T, S> {
    #[inline(always)]
    fn new(mutex: &'a SleepMutex<T, S>) -> Self {
        SleepMutexFuture {
            mutex,
            grant: Arc::new(GrantInfo {
                is_granted: AtomicBool::new(false),
                waker: UnsafeCell::new(None),
            }),
            queued: false,
            completed: false,
        }
    }
}

impl<'a, T: ?Sized, S: MutexOperations> Future for SleepMutexFuture<'a, T, S> {
    type Output = SleepMutexGuard<'a, T, S>;
    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        if this.grant.is_granted.load(Ordering::Acquire) {
            trace!("[SleepMutexFuture::poll] granted");
            this.completed = true;
            return Poll::Ready(SleepMutexGuard { mutex: this.mutex });
        }

        let mut inner = this.mutex.lock.lock();
        if !inner.is_locked {
            inner.is_locked = true;
            this.grant.is_granted.store(true, Ordering::Release);
            trace!("[SleepMutexFuture::poll] lock acquired immediately");
            this.completed = true;
            return Poll::Ready(SleepMutexGuard { mutex: this.mutex });
        }

        unsafe {
            *this.grant.waker.get() = Some(cx.waker().clone());
        }
        if !this.queued {
            trace!("[SleepMutexFuture::poll] wait for lock...");
            inner.wait_queue.push_back(this.grant.clone());
            this.queued = true;
        }

        Poll::Pending
    }
}

impl<'a, T: ?Sized, S: MutexOperations> Drop for SleepMutexFuture<'a, T, S> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }

        let mut inner = self.mutex.lock.lock();
        if self.grant.is_granted.load(Ordering::Acquire) {
            wake_next_or_unlock(&mut inner);
            return;
        }

        if !self.queued {
            return;
        }

        if let Some(index) = inner
            .wait_queue
            .iter()
            .position(|waiter| Arc::ptr_eq(waiter, &self.grant))
        {
            inner.wait_queue.remove(index);
        }
    }
}

struct SleepMutexGuard<'a, T: ?Sized, S: MutexOperations> {
    mutex: &'a SleepMutex<T, S>,
}

unsafe impl<'a, T: ?Sized + Send, S: MutexOperations> Send for SleepMutexGuard<'a, T, S> {}
unsafe impl<'a, T: ?Sized + Sync, S: MutexOperations> Sync for SleepMutexGuard<'a, T, S> {}

impl<'a, T: ?Sized, S: MutexOperations> Deref for SleepMutexGuard<'a, T, S> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexOperations> DerefMut for SleepMutexGuard<'a, T, S> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized, S: MutexOperations> Drop for SleepMutexGuard<'a, T, S> {
    #[inline]
    fn drop(&mut self) {
        trace!("[SleepMutexGuard::drop] drop...");
        let mut inner = self.mutex.lock.lock();
        debug_assert!(inner.is_locked);
        wake_next_or_unlock(&mut inner);
    }
}
