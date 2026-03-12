// os/src/task/context.rs

/// 任务上下文
/// 
/// - 功能：TODO 可能是内核异常控制流的上下文
/// - 参数
///     - `ra` 返回地址
///     - `sp` 栈指针
///     - `s` 临时寄存器组
#[derive(Copy, Clone)]
#[repr(C)]
pub struct TaskContext {
    ra: usize,
    sp: usize,
    s: [usize; 12], // 调用约定被调用者保存的寄存器
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

    /// 创建用于恢复指定内核栈上用户程序的上下文的任务上下文
    pub fn create_for_kstack_restore(kernel_stack_ptr: usize) -> Self {
        unsafe extern "C" {
            unsafe fn __restore();
        }
        Self {
            ra: __restore as *const() as usize,
            sp: kernel_stack_ptr,
            s: [0; 12],
        }
    }
}
