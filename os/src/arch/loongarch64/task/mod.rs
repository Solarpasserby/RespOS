// os/src/arch/loongarch64/task/mod.rs

use core::arch::global_asm;

global_asm!(include_str!("switch.S"));

unsafe extern "C" {
    /// 切换任务上下文
    ///
    /// 保存当前任务的寄存器到内核栈上，
    /// 从 next_task_kstack_ptr 恢复下一个任务的上下文。
    ///
    /// 注意：LA64 通过 `$tp` 寄存器传递当前任务指针（而非第二个参数），
    /// `_current_task_ptr` 在汇编中被忽略，仅为与 RV64 调用侧兼容而保留。
    pub fn __switch(next_task_kstack_ptr: usize, _current_task_ptr: usize);
}
