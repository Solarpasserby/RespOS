// 系统调用相关配置

pub const USER_CSTR_MAX_LEN: usize = 4096; // 用户态字符串最大长度
pub const USER_ARG_MAX_COUNT: usize = 4096; // argv/envp 指针数上限
pub const USER_ARG_MAX_BYTES: usize = 1024 * 1024; // argv 或 envp 的字符串总量上限

/// 用户态 sigreturn 跳板代码。
///
/// LoongArch: `addi.w $a7, $zero, 139; syscall 0`，用于从用户态信号处理函数
/// 返回后进入 `sys_sigreturn`。
pub const TRAMPOLINE_CODE: &[u8] = &[
    0x0b, 0x2c, 0x82, 0x03, // li.w $a7, 139
    0x00, 0x00, 0x2b, 0x00, // syscall 0
];
