// os/src/config.rs

//! ### 内核主要配置模块

// 内核终止地址
pub const KERNEL_MEM_END: usize = 0x80800000;

// 跳板虚拟地址
pub const TRAMPOLINE: usize = usize::MAX - PAGE_SIZE + 1;

// 用户程序异常上下文虚拟地址
pub const TRAP_CONTEXT: usize = TRAMPOLINE - PAGE_SIZE;

// 栈大小
pub const USER_STACK_SIZE: usize = 4096 * 2;
pub const KERNEL_STACK_SIZE: usize = 4096 * 2;

// 内核堆大小
pub const KERNEL_HEAP_SIZE: usize = 1_000_000;

// 页大小
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;

// 时钟频率，与硬件设备相关
pub const CLOCK_FREQ: usize = 12500000;   
