// os/src/mm/frame_allocator

use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;
use crate::config::{KERNEL_BASE, MEMORY_END};
use super::address::{PhysAddr, PhysPageNum};

type FrameAllocatorImpl = StackFrameAllocator;
lazy_static! {
    pub static ref FRAME_ALLOCATOR: Mutex<FrameAllocatorImpl> =
        Mutex::new(FrameAllocatorImpl::new());
}

/// 物理页帧追踪器
/// 
/// 分配物理页帧后的实体，主要用于追踪分配的物理页帧
/// 当页帧不再使用后（离开作用域）自动调用 `drop` 回收，省去了亲自回收物理页帧
#[derive(Debug)]
pub struct FrameTracker { // 感觉非常机智，需多利用类似的 Rust 提供的机制
    ppn: PhysPageNum,
}

/// 物理页帧分配器接口
/// 
/// 重写对于物理页内部数据的访问接口，现在内部数据生命周期与 [`FrameTracker`] 绑定，更加安全
impl FrameTracker {
    pub fn new(ppn: PhysPageNum) -> Self {
        let mut frame = Self { ppn };
        // 清空页帧，避免数据泄露
        frame.clear();
        frame
    }

    pub fn ppn(&self) -> PhysPageNum {
        self.ppn
    }

    fn clear(&mut self) {
        for byte in self.ppn.get_bytes_array() {
            *byte = 0;
        }
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        FRAME_ALLOCATOR
            .lock()
            .dealloc(self.ppn());
    }
}

/// 物理页帧分配器规范
trait FrameAllocator {
    fn new() -> Self;
    fn alloc(&mut self) -> Option<PhysPageNum>;
    fn dealloc(&mut self, ppn: PhysPageNum);
}

/// 栈式物理页帧分配器
/// 
/// 使用栈式页帧管理
pub struct StackFrameAllocator {
    current: usize,
    end: usize,
    recycled: Vec<usize>,
}

impl FrameAllocator for StackFrameAllocator {
    fn new() -> Self {
        Self {
            current: 0,
            end: 0,
            recycled: Vec::new(),
        }
    }

    fn alloc(&mut self) -> Option<PhysPageNum> {
        // `recycle` 中的值是页表页数数字，而输出要求为页表页数结构体
        if let Some(ppn) = self.recycled.pop() {
            Some(ppn.into())
        } else {
            if self.current == self.end {
                None
            } else {
                self.current += 1;
                Some((self.current - 1).into())
            }
        }
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0; // 使用了变量遮蔽，需要处理意外情况
        if ppn >= self.current || self.recycled
            .iter()
            .find(|&v| { *v == ppn })
            .is_some() {
            panic!("Failed to release frame ppn={:#?}", ppn);
        }
        self.recycled.push(ppn);
    }
}

impl StackFrameAllocator {
    fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.current = l.0;
        self.end = r.0;
    }
}

/// 初始化物理页帧分配器
/// 
/// 分配的是 qemu 中真实的物理地址
pub fn init_frame_allocator() {
    unsafe extern "C" {
        unsafe fn ekernel();
    }
    FRAME_ALLOCATOR.lock().init(
        PhysAddr::from(ekernel as *const() as usize - KERNEL_BASE).ceil(), 
        PhysAddr::from(MEMORY_END).floor(),
    );
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .lock()
        .alloc()
        .map(|ppn| FrameTracker::new(ppn))
}

#[allow(unused)]
pub fn frame_allocator_test() {
    let mut v: Vec<FrameTracker> = Vec::new();
    for i in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        v.push(frame);
    }
    v.clear();
    for i in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        v.push(frame);
    }
    drop(v);
    println!("frame_allocator_test passed!");
}
