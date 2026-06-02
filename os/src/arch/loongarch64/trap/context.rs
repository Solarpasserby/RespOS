// os/src/arch/loongarch64/trap/context.rs

/// LoongArch 异常上下文
///
/// 保存用户程序的完整执行状态：
/// - 32 个通用寄存器 (`x[0..31]`)
/// - `prmd`: 异常前的处理器模式（替代 RISC-V 的 sstatus）
/// - `era`: 异常返回地址（替代 RISC-V 的 sepc）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub prmd: usize, // Previous Mode: PPLV(bits 0..=1) + PIE(bit 2)
    pub era: usize,  // Exception Return Address
}

impl TrapContext {
    pub fn get_sp(&self) -> usize {
        self.x[3]
    }
    pub fn set_ra(&mut self, ra: usize) {
        self.x[1] = ra;
    }
    pub fn set_sp(&mut self, sp: usize) {
        self.x[3] = sp;
    }
    pub fn set_tp(&mut self, tp: usize) {
        self.x[2] = tp;
    }
    pub fn set_a0(&mut self, a0: usize) {
        self.x[4] = a0; // LoongArch a0 = r4
    }
    pub fn get_sepc(&self) -> usize {
        self.era
    }
    pub fn set_sepc(&mut self, sepc: usize) {
        self.era = sepc;
    }
    /// 获取 syscall id（LoongArch: a7 = r11）
    pub fn syscall_id(&self) -> usize {
        self.x[11]
    }
    /// 获取 syscall 参数（LoongArch: a0-a5 = r4-r9）
    pub fn syscall_args(&self) -> [usize; 6] {
        [
            self.x[4], self.x[5], self.x[6], self.x[7], self.x[8], self.x[9],
        ]
    }

    /// 初始化用户程序上下文
    pub fn init_app_context(
        entry: usize,
        sp: usize,
        argc: usize,
        argv_base: usize,
        envp_base: usize,
        auxv_base: usize,
        linux_abi: bool,
    ) -> Self {
        let mut regs = [0; 32];
        if linux_abi {
            // Linux LoongArch 进程入口从用户栈读取 argc/argv/envp/auxv。
            // glibc 的 _start 把 a0 当成 rtld_fini；静态程序这里必须传 0。
            regs[4] = 0;
        } else {
            // LoongArch ABI: a0=r4, a1=r5, a2=r6, a3=r7
            regs[4] = argc;
            regs[5] = argv_base;
            regs[6] = envp_base;
            regs[7] = auxv_base;
        }
        // PRMD: PPLV=3(User), PIE=1(异常返回后开启中断)
        let prmd = (3 << 0) | (1 << 2);
        let mut cx = Self {
            x: regs,
            prmd,
            era: entry,
        };
        cx.set_sp(sp);
        cx
    }
}
