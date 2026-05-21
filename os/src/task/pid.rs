// os/src/task/pid.rs

use lazy_static::lazy_static;
use spin::Mutex;
use alloc::vec::Vec;

lazy_static! {
    static ref PID_ALLOCATOR: Mutex<PidAllocatr> = Mutex::new(PidAllocatr::new());
}

pub struct PidHandle(pub usize);

impl Drop for PidHandle {
    fn drop(&mut self) {
        PID_ALLOCATOR.lock().dealloc(self.0);
    }
}

/// ~~进程~~任务号分配器
struct PidAllocatr {
    current: usize,
    recycled: Vec<usize>,
}

impl PidAllocatr {
    pub fn new() -> Self {
        PidAllocatr {
            current: 0,
            recycled: Vec::new(),
        }
    }

    pub fn alloc(&mut self) -> PidHandle {
        if let Some(pid) = self.recycled.pop() {
            PidHandle(pid)
        } else {
            self.current += 1;
            PidHandle(self.current - 1)
        }
    }

    pub fn dealloc(&mut self, pid: usize) {
        assert!(pid < self.current);
        assert!(
            !self.recycled.iter().any(|ppid| *ppid == pid),
            "pid {} has been deallocated!", pid
        );
        self.recycled.push(pid);
    }
}

pub fn pid_alloc() -> PidHandle {
    PID_ALLOCATOR.lock().alloc()
}
