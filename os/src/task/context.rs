// os/src/task/context.rs

use crate::trap::__restore;

/// 任务上下文
///
/// - 功能：存储任务的运行状态，然而由于切换任务使用函数调用实现，因此上下文内容更贴近被调用者运行时的内容
/// - 参数
///     - `ra` 返回地址
///     - `sp` 栈指针
///     - `s` 临时寄存器组
///     - `mmu_token` 页表状态寄存器
#[derive(Copy, Clone)]
#[repr(C)]
pub struct TaskContext {
    ra: usize,
    sp: usize,
    // 调用约定被调用者保存的寄存器
    // 由于切换上下文总是以函数调用的形式实现，因而只作部分保存
    s: [usize; 12],
    // 现在页表切换不在异常处理后进行，而是切换任务（进程）后进行
    mmu_token: usize,
}

impl TaskContext {
    /// 创建用于恢复指定内核栈上用户异常上下文的任务上下文
    pub fn app_init_task_context(kernel_stack_ptr: usize, token: usize) -> Self {
        Self {
            ra: __restore as *const () as usize,
            sp: kernel_stack_ptr,
            s: [0; 12],
            mmu_token: token,
        }
    }

    pub fn set_sp(&mut self, sp: usize) {
        self.sp = sp;
    }
    pub fn set_mmu_token(&mut self, token: usize) {
        self.mmu_token = token;
    }
}
