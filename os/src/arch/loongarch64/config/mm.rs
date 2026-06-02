//! LoongArch 内存布局

// 内核基地址：LA64 采用 48-bit canonical VA 时，高半区从这里开始。
pub const KERNEL_BASE: usize = 0xffff_8000_0000_0000;

// 用户栈大小（每个用户程序）
pub const USER_STACK_SIZE: usize = PAGE_SIZE << 7;

// 内核栈设置
pub const KERNEL_STACK_TOP: usize = 0xffff_ffff_ffff_f000;
pub const KERNEL_STACK_SIZE: usize = (PAGE_SIZE << 4) - PAGE_SIZE; // 60 KiB

// 内核堆大小
pub const KERNEL_HEAP_SIZE: usize = 8 * 1024 * 1024;

// 文件映射和匿名映射区域
pub const MMAP_MIN_ADDR: usize = 0x0000_0020_0000_0000;
pub const MMAP_MAX_ADDR: usize = 0x0000_0022_0000_0000;
pub const MMAP_AREA_SIZE: usize = MMAP_MAX_ADDR - MMAP_MIN_ADDR; // 8 GiB

// 页大小
pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_BITS: usize = 12;

/// 动态链接器（ld-linux）的加载基址偏移。
pub const DL_INTERP_OFFSET: usize = 0x30_0000_0000;
/// times() 系统调用的时钟滴答频率（Linux ABI 标准值 100Hz）。
pub const CLK_TCK: usize = 100;

/// 用户态 sigreturn 跳板页的虚拟地址。
///
/// LA64 当前按 48-bit 用户低半区布局，保留低半区最高附近的一页给信号返回跳板。
pub const TRAMPOLINE: usize = 0x0000_7fff_ffff_e000;
