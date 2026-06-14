// os/src/arch/rv64/config/mm.rs

//! 内存布局

// 内核基地址——内核线性映射到高地址
pub const KERNEL_BASE: usize = 0xffff_ffc0_0000_0000;
// pub const KERNEL_PN_OFFSET: usize = KERNEL_BASE >> PAGE_SIZE_BITS;
// 用户程序地址上界
// pub const USER_MAX: usize = 0x0000_003f_ffff_ffff;

// 栈大小，当前每个用户程序都有对应的内核栈
pub const USER_STACK_SIZE: usize = PAGE_SIZE << 7;

// 内核栈设置
pub const KERNEL_STACK_TOP: usize = 0xffff_ffff_ffff_f000;
pub const KERNEL_STACK_SIZE: usize = (PAGE_SIZE << 4) - PAGE_SIZE;
// 内核堆大小
pub const KERNEL_HEAP_SIZE: usize = 64 * 1024 * 1024;

// 文件映射和匿名映射区域
pub const MMAP_MIN_ADDR: usize = 0x0000_0020_0000_0000;
pub const MMAP_MAX_ADDR: usize = 0x0000_0022_0000_0000;
pub const MMAP_AREA_SIZE: usize = MMAP_MAX_ADDR - MMAP_MIN_ADDR; // 8G 大小

// 页大小
pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_BITS: usize = 12;
/// 动态链接器（ld-linux）的加载基址偏移
pub const DL_INTERP_OFFSET: usize = 0x30_0000_0000;
/// times() 系统调用的时钟滴答频率（Linux ABI 标准值 100Hz）
pub const CLK_TCK: usize = 100;

/// 用户态 sigreturn 跳板页的虚拟地址。
/// 该页在所有用户进程的地址空间中映射到同一位置。
///
/// Sv39 低半区最高一页的 end VPN 会跨到另一半区，和当前 VPNRange 的同半区
/// 检查冲突，因此这里保留最高一页不用。
pub const TRAMPOLINE: usize = 0x0000_003f_ffff_e000;
