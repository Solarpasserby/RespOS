// os/src/task/switch.rs

use core::arch::global_asm;
use super::context::TaskContext;

global_asm!(include_str!("switch.S"));

unsafe extern "C" {
    pub unsafe fn __switch(
        current_task_context_ptr: *mut TaskContext,
        next_task_context_ptr: *const TaskContext
    );
}