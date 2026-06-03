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
        // Keep task ids monotonic for now. Reusing a tid immediately after a
        // thread exits can race with weak task-manager entries and futex wakeup
        // paths that are still keyed by tid.
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
