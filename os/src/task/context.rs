// os/src/task/context.rs

use crate::trap::trap_return;

/// 任务上下文
/// 
/// - 功能：存储任务的运行状态，然而由于切换任务使用函数调用实现，因此上下文内容更贴近被调用者运行时的内容
/// - 参数
///     - `ra` 返回地址
///     - `sp` 栈指针
///     - `s` 临时寄存器组
#[derive(Copy, Clone)]
#[repr(C)]
pub struct TaskContext {
    ra: usize,
    sp: usize,
    // 调用约定被调用者保存的寄存器
    // 由于切换上下文总是以函数调用的形式实现，因而只作部分保存
    s: [usize; 12],
}

impl TaskContext {
    /// 全零初始化
    pub fn init_zero() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
        }
    }

    /// 创建用于恢复指定内核栈上用户异常上下文的任务上下文
    pub fn create_for_kstack_restore(kernel_stack_ptr: usize) -> Self {
        Self {
            ra: trap_return as *const() as usize,
            sp: kernel_stack_ptr,
            s: [0; 12],
        }
    }
}
