// os/src/trap/context.rs

use riscv::register::sstatus::{self, SPP, Sstatus};

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
    pub fn get_sp(&self) -> usize {
        self.x[2]
    }
    pub fn set_ra(&mut self, ra: usize) {
        self.x[1] = ra;
    }
    pub fn set_sp(&mut self, sp: usize) {
        self.x[2] = sp;
    }
    pub fn set_tp(&mut self, tp: usize) {
        self.x[4] = tp;
    }
    pub fn set_a0(&mut self, a0: usize) {
        self.x[10] = a0;
    }
    pub fn set_sepc(&mut self, sepc: usize) {
        self.sepc = sepc;
    }

    /// 初始化用户程序上下文
    pub fn init_app_context(
        entry: usize,
        sp: usize,
        argc: usize,
        argv_base: usize,
        envp_base: usize,
        auxv_base: usize,
    ) -> Self {
        let mut sstatus = sstatus::read(); // CSR sstatus
        sstatus.set_spp(SPP::User); //previous privilege mode: user mode
        let mut gerneal_regs = [0; 32];
        gerneal_regs[10] = argc;
        gerneal_regs[11] = argv_base;
        gerneal_regs[12] = envp_base;
        gerneal_regs[13] = auxv_base;
        let mut cx = Self {
            x: gerneal_regs,
            sstatus,
            sepc: entry,
        };
        // 设置用户栈顶指针
        cx.set_sp(sp);

        cx
    }
}
