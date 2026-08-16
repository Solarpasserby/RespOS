// os/src/mm/frame_allocator

use super::address::{PhysAddr, PhysPageNum};
use crate::config::{physical_memory_end, MEMORY_START};
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

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
pub struct FrameTracker {
    // 感觉非常机智，需多利用类似的 Rust 提供的机制
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
        let started = crate::perf::now_ticks();
        let mut allocator = FRAME_ALLOCATOR.lock();
        let locked = crate::perf::now_ticks();
        allocator.dealloc(self.ppn());
        let operated = crate::perf::now_ticks();
        drop(allocator);
        crate::perf::frame_dealloc(
            crate::perf::elapsed_since(started),
            locked.wrapping_sub(started),
            operated.wrapping_sub(locked),
        );
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
    ranges: Vec<FrameRange>,
    active_range: usize,
    recycled: Vec<usize>,
    recycled_bitmap: Vec<bool>,
}

struct FrameRange {
    start: usize,
    current: usize,
    end: usize,
}

impl FrameAllocator for StackFrameAllocator {
    fn new() -> Self {
        Self {
            ranges: Vec::new(),
            active_range: 0,
            recycled: Vec::new(),
            recycled_bitmap: Vec::new(),
        }
    }

    fn alloc(&mut self) -> Option<PhysPageNum> {
        // `recycle` 中的值是页表页数数字，而输出要求为页表页数结构体
        if let Some(ppn) = self.recycled.pop() {
            self.recycled_bitmap[ppn] = false;
            Some(ppn.into())
        } else {
            while let Some(range) = self.ranges.get_mut(self.active_range) {
                if range.current < range.end {
                    let ppn = range.current;
                    range.current += 1;
                    return Some(ppn.into());
                }
                self.active_range += 1;
            }
            None
        }
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0; // 使用了变量遮蔽，需要处理意外情况
        let was_allocated = self
            .ranges
            .iter()
            .any(|range| ppn >= range.start && ppn < range.current);
        if !was_allocated || self.recycled_bitmap.get(ppn).copied().unwrap_or(false) {
            panic!("Failed to release frame ppn={:#?}", ppn);
        }
        self.recycled_bitmap[ppn] = true;
        self.recycled.push(ppn);
    }
}

impl StackFrameAllocator {
    fn init(&mut self, ranges: &[(PhysPageNum, PhysPageNum)]) {
        self.ranges = ranges
            .iter()
            .filter(|(start, end)| start.0 < end.0)
            .map(|(start, end)| FrameRange {
                start: start.0,
                current: start.0,
                end: end.0,
            })
            .collect();
        self.active_range = 0;
        let max_ppn = self.ranges.iter().map(|range| range.end).max().unwrap_or(0);
        self.recycled_bitmap = alloc::vec![false; max_ppn];
    }

    pub fn free_frames(&self) -> usize {
        self.ranges
            .iter()
            .map(|range| range.end - range.current)
            .sum::<usize>()
            + self.recycled.len()
    }
}

/// 初始化物理页帧分配器
///
/// 分配的是 qemu 中真实的物理地址
pub fn init_frame_allocator(reserved_end: usize) {
    #[cfg(target_arch = "riscv64")]
    let ranges = [(
        PhysAddr::from(reserved_end.max(MEMORY_START)).ceil(),
        PhysAddr::from(physical_memory_end()).floor(),
    )];

    #[cfg(target_arch = "loongarch64")]
    let ranges = {
        unsafe extern "C" {
            unsafe fn ekernel();
        }
        let kernel_end = ekernel as *const () as usize - crate::config::KERNEL_BASE;
        [
            (
                PhysAddr::from(kernel_end.max(MEMORY_START)).ceil(),
                PhysAddr::from(crate::config::LOW_MEMORY_END).floor(),
            ),
            (
                PhysAddr::from(reserved_end.max(crate::config::HIGH_MEMORY_START)).ceil(),
                PhysAddr::from(physical_memory_end()).floor(),
            ),
        ]
    };

    FRAME_ALLOCATOR.lock().init(&ranges);
}

pub fn frame_alloc() -> Option<FrameTracker> {
    let started = crate::perf::now_ticks();
    let mut allocator = FRAME_ALLOCATOR.lock();
    let locked = crate::perf::now_ticks();
    let ppn = allocator.alloc();
    let operated = crate::perf::now_ticks();
    drop(allocator);

    let frame = ppn.map(|ppn| FrameTracker::new(ppn));
    let finished = crate::perf::now_ticks();
    crate::perf::frame_alloc(
        finished.wrapping_sub(started),
        locked.wrapping_sub(started),
        operated.wrapping_sub(locked),
        finished.wrapping_sub(operated),
        frame.is_some(),
    );
    frame
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
