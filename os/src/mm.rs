// os/src/mm.rs

//! ### 内存管理模块
//! 
//! 实现虚拟地址空间
//! 
//! 这部分内容繁多，建立了多层的抽象，隐含了很多深远的设计思想，需要好好消化

mod heap_allocator;
mod frame_allocator;
mod address;
mod page_table;
mod memory_set;

use heap_allocator::init_heap;
use frame_allocator::init_frame_allocator;
pub use address::*;
pub use frame_allocator::{ FrameTracker, frame_alloc };
pub use page_table::{ PageTableEntry, PageTable };
pub use memory_set::{ MemorySet, MapArea };

/// 初始化内存管理
pub fn init() {
    init_heap();
    init_frame_allocator();
}