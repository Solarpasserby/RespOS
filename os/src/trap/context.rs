// os/src/trap/context.rs

use riscv::register::sstatus::{ self, Sstatus };

/// 异常上下文
/// 
/// - 功能：用于保存用户程序的执行状态和内核态相关信息
/// - 内容:
///     - `x` 通用寄存器组
///     - `sstatus` 返回特权级
///     - `spec` 异常程序计数器
///     - `kernel_satp` 内核地址空间的 token
///     - `kernel_sp` 内核栈指针
///     - `trap_handler` 异常处理程序地址
/// 
/// - 注意：这里的设计将用程序上下文存放于用户空间中，与之前不太一致。我还不太懂
#[repr(C)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: Sstatus,
    pub sepc: usize,
    pub kernel_sp: usize,
}

impl TrapContext {
    /// 初始化用户程序上下文
    /// 
    /// - 参数：
    ///     - `entry` 用户程序入口
    ///     - `sp` 用户程序栈
    ///     - `kernel_satp` 内核地址空间的 token
    ///     - `kernel_sp` 内核栈指针
    ///     - `trap_handler` 异常处理程序地址
    pub fn init_app_context(
        entry: usize,
        sp: usize,
        kernel_satp: usize,
        kernel_sp: usize,
        trap_handler: usize,
    ) -> Self {
        let mut sstatus = sstatus::read();
        sstatus.set_spp(sstatus::SPP::User);
        let mut context = Self {
            x: [0; 32],
            sstatus,
            sepc: entry,
            kernel_satp,
            kernel_sp,
            trap_handler,
        };
        context.x[2] = sp;
        context
    }
}