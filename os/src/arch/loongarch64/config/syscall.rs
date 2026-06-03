// 系统调用相关配置

pub const USER_CSTR_MAX_LEN: usize = 4096; // 用户态字符串最大长度
pub const USER_ARG_MAX_COUNT: usize = 32; // 用户态命令行参数最大数量

/// 用户态 sigreturn 跳板代码。
///
/// LoongArch: `addi.w $a7, $zero, 139; syscall 0`，用于从用户态信号处理函数
/// 返回后进入 `sys_sigreturn`。
pub const TRAMPOLINE_CODE: &[u8] = &[
    0x0b, 0x2c, 0x82, 0x02, // addi.w $a7, $zero, 139
    0x00, 0x80, 0x15, 0x00, // syscall 0
];
