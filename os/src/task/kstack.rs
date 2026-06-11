// os/src/task/kstack.rs

use super::tid::TidHandle;
use crate::config::{KERNEL_STACK_SIZE, KERNEL_STACK_TOP, PAGE_SIZE};
use crate::mm::KERNEL_SPACE;
use alloc::vec::Vec;
use core::cell::SyncUnsafeCell;
use lazy_static::lazy_static;
use spin::Mutex;

lazy_static! {
    static ref KERNEL_STACK_ALLOCATOR: Mutex<KernelStackAllocator> =
        Mutex::new(KernelStackAllocator::new());
}

/// 内核栈 slot 分配器。
///
/// 每个 slot 对应内核栈区域中一个固定位置：
/// `KERNEL_STACK_TOP - slot * (KERNEL_STACK_SIZE + PAGE_SIZE)`，
/// 相邻 slot 之间有一个守卫页面。
///
/// slot 在任务退出时回收并复用，避免栈地址随 tid 增长持续下探，
/// 从而防止跨 PMD 边界引发的页表异常。
struct KernelStackAllocator {
    current: usize,
    /// 已回收的空闲 slot，LIFO 复用热 slot，提高 TLB 命中率
    recycled: Vec<usize>,
}

impl KernelStackAllocator {
    fn new() -> Self {
        Self {
            current: 0,
            recycled: Vec::new(),
        }
    }

    fn alloc(&mut self) -> usize {
        // 优先复用已回收的 slot，低地址优先（热 slot）
        if let Some(slot) = self.recycled.pop() {
            slot
        } else {
            let slot = self.current;
            self.current += 1;
            slot
        }
    }

    fn dealloc(&mut self, slot: usize) {
        // 防止 double-free：同一个 slot 被回收两次表示逻辑错误
        assert!(
            !self.recycled.iter().any(|&recycled| recycled == slot),
            "kernel stack slot {} has been deallocated!",
            slot
        );
        self.recycled.push(slot);
    }
}

/// 内核栈
///
/// 功能：为用户进程提供的需要内核功能服务场景下的数据缓存，主要体现为异常处理
///
/// 虽然设计上该内核栈本身不存储数据（存储于内核空间 [`KERNEL_SPACE`]）
/// 但是在内核栈生命周期结束后，其对应内核空间的内存也会被释放，体现了 RAII 的思想
#[repr(C)]
pub struct KernelStack {
    // `__switch` 会通过 TaskControlBlock 的起始地址直接写回这个字段。
    // 这里必须具备内部可变性，否则 release 优化下普通共享字段被汇编修改属于未定义行为。
    top: SyncUnsafeCell<usize>, // 内核栈顶指针
    slot: usize,
}

impl KernelStack {
    pub fn zero_init() -> Self {
        Self {
            top: SyncUnsafeCell::new(0),
            slot: 0,
        }
    }

    pub fn new(tid_handle: &TidHandle) -> Self {
        let _tid = tid_handle.0;
        let slot = KERNEL_STACK_ALLOCATOR.lock().alloc();
        let stack_top = get_kernel_stack_top_edge(slot);
        KERNEL_SPACE.lock().insert_stack_area(stack_top);
        Self {
            top: SyncUnsafeCell::new(stack_top),
            slot,
        }
    }

    pub fn get_top_edge(&self) -> usize {
        get_kernel_stack_top_edge(self.slot)
    }

    pub fn get_top(&self) -> usize {
        unsafe { *self.top.get() }
    }
    pub fn set_top(&mut self, stack_top: usize) {
        *self.top.get_mut() = stack_top;
    }

    #[allow(unused)]
    pub fn push_on_top<T>(&self, value: T) -> *mut T
    where
        T: Sized,
    {
        let kernel_top = self.get_top();
        let ptr_mut = (kernel_top - core::mem::size_of::<T>()) as *mut T;
        unsafe {
            *ptr_mut = value;
        }
        ptr_mut
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        if *self.top.get_mut() == 0 {
            return;
        }
        KERNEL_SPACE
            .lock()
            .remove_stack_area(get_kernel_stack_top_edge(self.slot));
        KERNEL_STACK_ALLOCATOR.lock().dealloc(self.slot);
    }
}

/// 获取内核栈顶地址，保留守卫页面
fn get_kernel_stack_top_edge(slot: usize) -> usize {
    KERNEL_STACK_TOP - slot * (KERNEL_STACK_SIZE + PAGE_SIZE)
}
