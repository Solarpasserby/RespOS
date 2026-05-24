// os/src/task/kstack.rs

use super::tid::TidHandle;
use crate::config::{KERNEL_STACK_SIZE, KERNEL_STACK_TOP, PAGE_SIZE};
use crate::mm::KERNEL_SPACE;

/// 内核栈
///
/// 功能：为用户进程提供的需要内核功能服务场景下的数据缓存，主要体现为异常处理
///
/// 虽然设计上该内核栈本身不存储数据（存储于内核空间 [`KERNEL_SPACE`]）
/// 但是在内核栈生命周期结束后，其对应内核空间的内存也会被释放，体现了 RAII 的思想
#[repr(C)]
pub struct KernelStack {
    top: usize, // 内核栈顶指针
    tid: usize, // 标识用户线程
}

impl KernelStack {
    pub fn zero_init() -> Self {
        Self { top: 0, tid: 0 }
    }

    pub fn new(tid_handle: &TidHandle) -> Self {
        let tid = tid_handle.0;
        let stack_top = get_kernel_stack_top_edge(tid);
        KERNEL_SPACE.lock().insert_stack_area(stack_top);
        Self {
            top: stack_top,
            tid,
        }
    }

    pub fn get_top_edge(&self) -> usize {
        get_kernel_stack_top_edge(self.tid)
    }

    pub fn get_top(&self) -> usize {
        self.top
    }
    pub fn set_top(&mut self, stack_top: usize) {
        self.top = stack_top;
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
        KERNEL_SPACE
            .lock()
            .remove_stack_area(get_kernel_stack_top_edge(self.tid));
    }
}

/// 获取内核栈顶地址，保留守卫页面
fn get_kernel_stack_top_edge(app_id: usize) -> usize {
    KERNEL_STACK_TOP - app_id * (KERNEL_STACK_SIZE + PAGE_SIZE)
}
