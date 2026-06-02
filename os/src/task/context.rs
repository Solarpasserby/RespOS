// os/src/task/context.rs

use crate::trap::__restore;

/// 任务上下文
///
/// 存储任务的运行状态，然而由于切换任务使用函数调用实现，因此上下文内容更贴近被调用者运行时的内容
#[derive(Copy, Clone)]
#[repr(C)]
pub struct TaskContext {
    ra: usize, // 返回地址
    tp: usize, // 线程指针寄存器，仅按调用约定保存/恢复
    // 调用约定被调用者保存的寄存器
    // 由于切换上下文总是以函数调用的形式实现，因而只作部分保存
    // RISC-V: s0-s11；LoongArch: fp/s9 + s0-s8 + padding
    s: [usize; 12],
    // 现在页表切换不在异常处理后进行，而是切换任务（进程）后进行
    mmu_token: usize,
    _padding: usize, // 用于对齐，只是为了和汇编对应
}

impl TaskContext {
    /// 创建用于恢复指定内核栈上用户异常上下文的任务上下文
    pub fn app_init_task_context(token: usize) -> Self {
        Self {
            ra: __restore as *const () as usize,
            tp: 0,
            s: [0; 12],
            mmu_token: token,
            _padding: 0,
        }
    }

    pub fn set_tp(&mut self, tp: usize) {
        self.tp = tp;
    }
    pub fn set_mmu_token(&mut self, token: usize) {
        self.mmu_token = token;
    }
}
