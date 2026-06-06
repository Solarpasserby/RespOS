use super::SigSet;

pub const SS_DISABLE: usize = 1;

#[derive(Clone, Copy, Debug)]
#[repr(C)]

// 用户通过 sigaltstack() 系统调用设置这个栈。然后注册 handler 时带上 SA_ONSTACK flag，内核就会在 ss_sp 上跑 handler 而不是普通用户栈

pub struct SignalStack {
    pub ss_sp: usize,   // 栈底
    pub ss_flags: i32,  // 是否启用  0 = 启用，1 = 禁用（SS_DISABLE）
    pub ss_size: usize, // 栈大小
}

impl Default for SignalStack {
    fn default() -> Self {
        SignalStack {
            ss_sp: 0,
            ss_flags: SS_DISABLE as i32,
            ss_size: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
// pub struct UContext {
//     // 保存具体机器状态的上下文信息，这是一个机器相关的表示，包含了处理器的寄存器状态等信息
//     pub uc_mcontext: SigContext,
//     // 标志位。目前只有一种情况：如果是 sigreturn 恢复的上下文，这个值为 1（表示这个 UContext是从信号处理中回来的）。大多数时候是 0
//     pub uc_flags: usize,
//     /// 指向前一个 UContext 的指针。信号是可以嵌套的：handler 运行期间又来了另一个信号 → 内核再压一个 SigContext →形成链表。uc_link 就是这个链表的指针，指向上一次的 UContext
//     pub uc_link: usize,
//     // 此上下文中阻塞的信号集
//     pub uc_sigmask: SigSet,
//     // 当前上下文使用的栈信息,包含栈的基址、大小等信息
//     pub uc_stack: SignalStack,
// }
// 修正后：对齐 Linux RISC-V ABI（字段顺序与 musl libc 的 __ucontext 一致）
#[repr(C)]
pub struct UContext {
    pub uc_flags: usize,         // offset 0
    pub uc_link: usize,          // offset 8
    pub uc_stack: SignalStack,   // offset 16 (24 bytes)
    pub uc_sigmask: SigSet,      // offset ~40 (8 bytes)
    pub uc_sig: [usize; 16],     // offset ~48 (128 bytes, 填充 sigset_t 空间)
    pub uc_mcontext: SigContext, // offset ~176 ← 这才是正确的 mcontext 位置！
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[cfg(target_arch = "riscv64")]
pub struct SigContext {
    // Linux/musl RISC-V mcontext_t starts with gregs[32], and gregs[0]
    // is the interrupted PC. musl's SIGCANCEL handler rewrites this slot.
    pub gregs: [usize; 32],
    pub mask: SigSet, // 记录原先的mask
    pub info: usize,  // 标志是否存在SIGINFO
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[cfg(target_arch = "loongarch64")]
pub struct SigContext {
    pub x: [usize; 32], // 32 个通用寄存器的值
    pub sepc: usize,    // 被中断的那条指令的地址
    pub mask: SigSet,   // 记录原先的mask
    pub info: usize,    // 标志是否存在SIGINFO
}
