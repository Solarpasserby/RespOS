// os/src/config/driver.rs

//! 内存布局

// 内核基地址——内核线性映射到高地址
pub const KERNEL_BASE: usize = 0xffff_ffc0_0000_0000;
pub const KERNEL_PN_OFFSET: usize = KERNEL_BASE >> PAGE_SIZE_BITS;
// 用户程序地址上界
// pub const USER_MAX: usize = 0x0000_003f_ffff_ffff;

// 栈大小，当前每个用户程序都有对应的内核栈
pub const USER_STACK_SIZE: usize = PAGE_SIZE * 2;

// 内核栈设置
pub const KERNEL_STACK_TOP: usize = 0xffff_ffff_ffff_f000;
pub const KERNEL_STACK_SIZE: usize = (PAGE_SIZE << 4) - PAGE_SIZE;
// 内核堆大小
pub const KERNEL_HEAP_SIZE: usize = 1_000_000;

// 页大小
pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_BITS: usize = 12;
