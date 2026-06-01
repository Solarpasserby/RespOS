// os/src/task/switch.rs

use core::arch::global_asm;

global_asm!(include_str!("switch.S"));

unsafe extern "C" {
    /// 切换任务上下文
    ///
    /// 该函数会修改当前任务的任务上下文 [`TaskContext`]，并修改 CPU 状态
    ///
    /// 如果当前任务调用了该函数，则从内核的角度来看当前任务被暂停了，转而执行另一个任务。
    /// 直到某个任务调用了该函数，修改 CPU 状态到当前任务完成该函数调用的状态，此时从内核的角度来看当前任务继续执行
    ///
    /// 因此，从任务本身的角度来看，调用该函数并没有发生任何事
    pub unsafe fn __switch(next_task_kstack_ptr: usize, current_task_ptr: usize);
}
