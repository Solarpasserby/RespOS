// os/src/mm.rs

//! ### 内存管理模块
//! 
//! 实现虚拟地址空间

mod heap_allocator;
mod frame_allocator;
mod address;
mod page_table;
mod memory_set;

pub use heap_allocator::init_heap;
pub use frame_allocator::*;
pub use address::*;
pub use page_table::{ PageTableEntry, PageTable };

/// 初始化内存管理
pub fn init() {
    init_heap();
    init_frame_allocator();
}