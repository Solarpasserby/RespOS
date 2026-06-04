// os/src/task/tid.rs

use alloc::vec::Vec;
use core::fmt::Display;
use lazy_static::lazy_static;
use spin::Mutex;

// 线程号分配器
lazy_static! {
    static ref TID_ALLOCATOR: Mutex<TidAllocator> = Mutex::new(TidAllocator::new());
}

pub struct TidHandle(pub usize);

impl Drop for TidHandle {
    fn drop(&mut self) {
        // TODO：
        // 暂时保持任务号单调递增。在线程退出后立刻复用 tid
        // 可能与 task-manager 中的 weak 条目及 futex wakeup 路径
        // 产生竞态——这些路径仍在使用 tid 作为索引键
        // 之后可以考虑在进程回收时统一回收线程号
    }
}

impl Display for TidHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// ~~进程~~任务号分配器
struct TidAllocator {
    current: usize,
    recycled: Vec<usize>,
}

impl TidAllocator {
    pub fn new() -> Self {
        TidAllocator {
            current: 0,
            recycled: Vec::new(),
        }
    }

    pub fn alloc(&mut self) -> TidHandle {
        if let Some(tid) = self.recycled.pop() {
            TidHandle(tid)
        } else {
            self.current += 1;
            TidHandle(self.current - 1)
        }
    }

    #[allow(dead_code)]
    pub fn dealloc(&mut self, tid: usize) {
        assert!(tid < self.current);
        assert!(
            !self.recycled.iter().any(|ptid| *ptid == tid),
            "tid {} has been deallocated!",
            tid
        );
        self.recycled.push(tid);
    }
}

pub fn tid_alloc() -> TidHandle {
    TID_ALLOCATOR.lock().alloc()
}
