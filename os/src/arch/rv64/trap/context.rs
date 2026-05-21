// os/src/trap/context.rs

use riscv::register::sstatus::{self, Sstatus};

/// 异常上下文
///
/// - 功能：用于保存用户程序的执行状态和内核态相关信息
/// - 内容:
///     - `x` 通用寄存器组
///     - `sstatus` 返回特权级
///     - `spec` 异常程序计数器
///
/// - 注意：这里的设计将用程序上下文存放于用户空间中，与之前不太一致。我还不太懂
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: Sstatus,
    pub sepc: usize,
}

impl TrapContext {
    pub fn set_sp(&mut self, sp: usize) {
        self.x[2] = sp;
    }
    pub fn set_tp(&mut self, tp: usize) {
        self.x[4] = tp;
    }
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
        context.set_sp(sp);
        context
    }
}
