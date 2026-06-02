// os/src/task/aux.rs

/// 内核向用户态动态链接器传递的辅助信息条目。
///
/// 格式遵循 ELF auxiliary vector 规范（Linux ABI），
/// 每一项由 `aux_type`（键）和 `value`（值）组成。
#[repr(C)]
#[derive(Copy, Clone)]
pub struct AuxHeader {
    pub aux_type: usize,
    pub value: usize,
}

// —— auxiliary vector 类型常量 ——

/// 向量结束标记
pub const AT_NULL: usize = 0;
/// 程序头表在内存中的虚拟地址
pub const AT_PHDR: usize = 3;
/// 程序头表中每个条目的大小（字节）
pub const AT_PHENT: usize = 4;
/// 程序头表中条目的数量
pub const AT_PHNUM: usize = 5;
/// 系统页大小（字节）
pub const AT_PAGESZ: usize = 6;
/// 动态链接器（ld-linux）的加载基址
pub const AT_BASE: usize = 7;
/// 可执行程序的入口点虚拟地址
pub const AT_ENTRY: usize = 9;
/// 进程的实际用户 ID
pub const AT_UID: usize = 11;
/// 进程的有效用户 ID
pub const AT_EUID: usize = 12;
/// 进程的实际组 ID
pub const AT_GID: usize = 13;
/// 进程的有效组 ID
pub const AT_EGID: usize = 14;
/// 标识 CPU 平台的字符串指针
pub const AT_PLATFORM: usize = 15;
/// times() 系统调用的时钟滴答频率
pub const AT_CLKTCK: usize = 17;
/// 16 字节随机数据的地址（用于栈保护 canary）
pub const AT_RANDOM: usize = 25;
/// 可执行文件的路径字符串指针
pub const AT_EXECFN: usize = 31;
