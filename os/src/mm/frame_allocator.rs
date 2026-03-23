// os/src/mm/frame_allocator

use alloc::vec::Vec;
use lazy_static::lazy_static;
use crate::config::KERNEL_MEM_END;
use crate::sync::UPSafeCell;
use super::address::{ PhysAddr, PhysPageNum };

type FrameAllocatorImpl = StackFrameAllocator;
lazy_static! {
    pub static ref FRAME_ALLOCATOR: UPSafeCell<FrameAllocatorImpl> = unsafe {
        UPSafeCell::new(FrameAllocatorImpl::new())
    };
}


/// 初始化物理页帧分配器
pub fn init_frame_allocator() {
    unsafe extern "C" {
        unsafe fn ekernel();
    }
    FRAME_ALLOCATOR
        .exclusive_access()
        .init(PhysAddr::from(ekernel as *const() as usize).ceil(), PhysAddr::from(KERNEL_MEM_END).floor());
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc()
        .map(|ppn| FrameTracker::new(ppn))
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

    // FIXME: 可能被优化了
    // /// 获取页表页内的页表项数组
    // pub fn pte_array(&self) -> &mut [PageTableEntry] {
    //     let pa = PhysAddr::from(self.ppn);
    //     unsafe {
    //         core::slice::from_raw_parts_mut(
    //             pa.0 as *mut PageTableEntry,
    //             crate::config::PAGE_SIZE / core::mem::size_of::<PageTableEntry>(),
    //         )
    //     }
    // }
    // /// 获取页帧头部的任意类型数据
    // pub fn get_head_mut<T>(&mut self) -> &mut T {
    //     let pa = PhysAddr::from(self.ppn);
    //     unsafe { &mut *(pa.0 as *mut T) }
    // }

    /// 获取页帧内的字节数组
    pub fn bytes_array(&mut self) -> &mut [u8] {
        let pa = PhysAddr::from(self.ppn);
        unsafe { core::slice::from_raw_parts_mut(pa.0 as *mut u8, crate::config::PAGE_SIZE) }
    }

    fn clear(&mut self) {
        for byte in self.bytes_array() {
            *byte = 0;
        }
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        FRAME_ALLOCATOR
            .exclusive_access()
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
