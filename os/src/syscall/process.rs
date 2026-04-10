// os/src/syscall/process.rs

use alloc::sync::Arc;
use crate::task::{
    current_task,
    current_user_token,
    add_task,
    exit_current_and_run_next,
    suspend_current_and_run_next,
};
use crate::loader::get_app_data_by_name;
use crate::mm::{ translate_str, translated_refmut };
use crate::timer::get_time_ms;

pub fn sys_exit(exit_code: i32) -> ! {
    // println!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}

pub fn sys_get_time() -> isize {
    get_time_ms() as isize
}

pub fn sys_fork() -> isize {
    let current_task = current_task().unwrap();
    // 此处发生任务复制
    let new_task = current_task.fork();
    let new_pid = new_task.pid();
    // 修改新任务的异常上下文，将其 sys_fork 的返回值设为 0
    let new_task_cx = new_task.inner_exclusive_access().get_trap_cx();
    new_task_cx.x[10] = 0;
    // 添加新任务
    add_task(new_task);
    // 系统调用返回新创建任务的 pid
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translate_str(token, path);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        0
    } else {
        -1
    }
}

/// 等待子任务结束
///
/// - 参数：
///     - `pid` 接受查询子任务任务号，可选值 -1 表示任意子任务
///     - `exit_code_ptr` 目标子任务的退出码
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    let task = current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();

    // 无法找到目标子任务则返回
    if task_inner.children.iter()
        .find(|p| pid == -1 || pid as usize == p.pid())
        .is_none() {
        return -1
    }
    // 得到目标子任务
    let pair = task_inner.children.iter()
        .enumerate()
        .find(|(_, p)| {
            p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.pid())
        });

    if let Some((idx, _)) = pair {
        let child = task_inner.children.remove(idx);
        // 子任务已经结束，应当已被回收，此时仅有其父任务拥有其原子引用
        assert_eq!(Arc::strong_count(&child), 1);
        let child_pid = child.pid();
        let exit_code = child.inner_exclusive_access().exit_code;
        *translated_refmut(task_inner.get_user_token(), exit_code_ptr) = exit_code;

        child_pid as isize
    } else { // 存在目标子任务但仍未结束，返回 -2
        -2
    }
}