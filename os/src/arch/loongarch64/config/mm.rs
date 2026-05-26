// LoongArch 内存布局配置
//
// 初始阶段运行在物理地址模式 (CRMD.DA=1)。
// KERNEL_BASE=0 表示无偏移，phys == virt。
// mm::init() 之后会建立页表和 DMW，切换到虚拟地址模式。

// 内核基地址 —— 初始为 0（物理模式），后续 mm::init 会建立映射
pub const KERNEL_BASE: usize = 0x0000_0000_0000_0000;

// 用户栈大小（每个用户程序）
pub const USER_STACK_SIZE: usize = PAGE_SIZE * 8;

// 内核栈设置
pub const KERNEL_STACK_TOP: usize = 0xffff_ffff_ffff_f000;
pub const KERNEL_STACK_SIZE: usize = (PAGE_SIZE << 4) - PAGE_SIZE; // 15 * PAGE_SIZE = 60 KiB

// 内核堆大小
pub const KERNEL_HEAP_SIZE: usize = 1_000_000;

// 文件映射和匿名映射区域
pub const MMAP_MIN_ADDR: usize = 0x0000_0020_0000_0000;
pub const MMAP_MAX_ADDR: usize = 0x0000_0022_0000_0000;
pub const MMAP_AREA_SIZE: usize = MMAP_MAX_ADDR - MMAP_MIN_ADDR; // 8G

// 页大小
pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_BITS: usize = 12;
