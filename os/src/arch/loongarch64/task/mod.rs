// os/src/arch/loongarch64/task/mod.rs

use core::arch::global_asm;

global_asm!(include_str!("switch.S"));

unsafe extern "C" {
    /// 切换任务上下文
    ///
    /// 保存当前任务的寄存器到内核栈上，
    /// 从 next_task_kstack_ptr 恢复下一个任务的上下文。
    ///
    /// `current_task_ptr` 用于写回当前任务的内核栈顶。
    pub fn __switch(next_task_kstack_ptr: usize, current_task_ptr: usize);
}
