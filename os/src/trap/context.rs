// os/src/trap/context.rs

use riscv::register::sstatus::{ self, Sstatus };

/// 异常上下文
/// 
/// - 功能：用于保存用户程序的执行状态
/// - 内容:
///     - `x` 通用寄存器组
///     - `sstatus` 返回特权级
///     - `spec` 异常程序计数器
#[repr(C)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: Sstatus,
    pub sepc: usize,
}

impl TrapContext {
    /// 初始化用户程序上下文
    /// 
    /// - 参数：
    ///     - `entry` 用户程序入口
    ///     - `sp` 用户程序栈
    pub fn init_app_context(entry: usize, sp: usize) -> Self {
        let mut sstatus = sstatus::read();
        sstatus.set_spp(sstatus::SPP::User);
        let mut context = Self {
            x: [0; 32],
            sstatus,
            sepc: entry,
        };
        context.x[2] = sp;
        context
    }
}