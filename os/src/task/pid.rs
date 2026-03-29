// os/src/task/pid.rs

use lazy_static::lazy_static;
use alloc::vec::Vec;
use crate::sync::UPSafeCell;
use crate::config::{ TRAMPOLINE, KERNEL_STACK_SIZE, PAGE_SIZE };
use crate::mm::KERNEL_SPACE;

lazy_static! {
    static ref PID_ALLOCATOR: UPSafeCell<PidAllocatr> = unsafe {
        UPSafeCell::new(PidAllocatr::new())
    };
}

pub struct PidHandle(pub usize);

impl Drop for PidHandle {
    fn drop(&mut self) {
        PID_ALLOCATOR.exclusive_access().dealloc(self.0);
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
            self.recycled.iter().find(|ppid| **ppid == pid).is_none(),
            "pid {} has been deallocated!", pid
        );
        self.recycled.push(pid);
    }
}

/// 内核栈
/// 
/// - 功能：为用户进程提供的需要内核功能服务场景下的数据缓存，主要体现为异常处理
/// - 参数：
///     - `pid`: 标识用户进程
/// 
/// 虽然设计上该内核栈本身不存储数据（存储于内核空间 [`KERNEL_SPACE`]）
/// 但是在内核栈生命周期结束后，其对应内核空间的内存也会被释放，体现了 RAII 的思想
pub struct KernelStack {
    pid: usize,
}

impl KernelStack {
    pub fn new(pid_handle: &PidHandle) -> Self {
        let pid = pid_handle.0;
        KERNEL_SPACE
            .exclusive_access()
            .insert_stack_area(get_kernel_stack_top(pid));
        KernelStack { pid, }
    }

    pub fn push_on_top<T>(&self, value: T) -> *mut T
        where T: Sized {
        let kernel_top = self.get_top();
        let ptr_mut = (kernel_top - core::mem::size_of::<T>()) as *mut T;
        unsafe { *ptr_mut = value; }
        ptr_mut
    }
    pub fn get_top(&self) -> usize {
        get_kernel_stack_top(self.pid)
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        KERNEL_SPACE
            .exclusive_access()
            .remove_stack_area(get_kernel_stack_top(self.pid));
    }
}

/// 获取内核栈顶地址，保留守卫页面
pub fn get_kernel_stack_top(app_id: usize) -> usize {
    TRAMPOLINE - app_id * (KERNEL_STACK_SIZE + PAGE_SIZE)
}

pub fn pid_alloc() -> PidHandle {
    PID_ALLOCATOR.exclusive_access().alloc()
}
