// os/src/config/syscall.rs

pub const USER_CSTR_MAX_LEN: usize = 4096; // 用户程序字符串长度限制
pub const USER_ARG_MAX_COUNT: usize = 32; // 用户程序命令行参数限制

/// 用户态 sigreturn 跳板代码。
///
/// RISC-V: `li a7, 139; ecall`，用于从用户态信号处理函数返回后进入
/// `sys_sigreturn`。
pub const TRAMPOLINE_CODE: &[u8] = &[
    0x93, 0x08, 0xb0, 0x08, // addi x17, x0, 139
    0x73, 0x00, 0x00, 0x00, // ecall
];
