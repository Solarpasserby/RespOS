//! LoongArch 内存布局

// 内核基地址：当前按 39-bit VA / 三级页表组织，和 rv64 的高地址共享内核
// 模型保持同一布局。
pub const KERNEL_BASE: usize = 0xffff_ffc0_0000_0000;

// Reserve the conventional Linux-sized 8 MiB user stack.  The VMA is lazy,
// so this is an address-space limit rather than an 8 MiB allocation per task.
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024;

// 内核栈设置
pub const KERNEL_STACK_TOP: usize = 0xffff_ffff_ffff_f000;
pub const KERNEL_STACK_SIZE: usize = PAGE_SIZE * 8; // 32 KiB

// 内核堆不属于 ELF/BSS；它位于 QEMU high RAM 开头。early page table 单独映射
// 该区域，正式页表则映射完整的 low/high RAM 并跳过 PCI/MMIO 空洞。User frames
// and page-cache frames are not allocated from this reserved heap.
pub const KERNEL_HEAP_SIZE: usize = 256 * 1024 * 1024;
/// Put the large heap in QEMU's high RAM instead of overflowing 256 MiB low RAM.
pub const KERNEL_HEAP_PHYS_START: usize = crate::config::HIGH_MEMORY_START;

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
/// 39-bit 用户低半区最高附近的一页保留给信号返回跳板。
pub const TRAMPOLINE: usize = 0x0000_003f_ffff_e000;
