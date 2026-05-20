// os/src/syscall/process.rs

use crate::loader::get_app_data_by_name;
use crate::task::{
    add_task, current_task, exit_current_and_run_next, suspend_current_and_run_next,
};
use alloc::sync::Arc;

use super::{Errno, SysResult};
use crate::fs::path_open;
use crate::mm::{copy_cstr_from_user, extract_cstrings_from_user};

pub fn sys_exit(exit_code: i32) -> ! {
    // println!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_sched_yield() -> SysResult<usize> {
    suspend_current_and_run_next();
    Ok(0)
}

pub fn sys_clone(
    _flags: usize,
    _stack: usize,
    _ptid: usize,
    _tls: usize,
    _ctid: usize,
) -> SysResult<usize> {
    // TODO[ABI-COMPAT]: 当前仅复用了 fork 语义，尚未真正支持 clone 的 flags/stack/tls 等能力。
    let current_task = current_task().expect("[kernel] current task is None.");
    // 此处发生任务复制
    let new_task = current_task.fork();
    let new_pid = new_task.pid();
    // 修改新任务的异常上下文，将其 sys_fork 的返回值设为 0
    let new_task_cx = new_task.get_trap_cx();
    new_task_cx.x[10] = 0;
    // 添加新任务
    add_task(new_task);
    // 系统调用返回新创建任务的 pid
    Ok(new_pid)
}

pub fn sys_execve(path: *const u8, args: *const usize, _envp: *const usize) -> SysResult<usize> {
    // TODO[ABI-COMPAT]: 当前忽略 envp，后续如需完整 execve 语义应补上环境变量处理。
    let path = copy_cstr_from_user(path)?;
    let args_vec = extract_cstrings_from_user(args)?;
    let task = current_task().expect("[kernel] current task is None.");

    if let Ok(file) = path_open(&path, 0, 0) {
        info!("[kernel] execute file in fs");
        let all_data = file.read_all()?;
        Ok(task.exec(all_data.as_slice(), args_vec)?)
    } else if !path.starts_with("/") {
        // 从内核中加载的应用程序
        if let Some(data) = get_app_data_by_name(path.as_str()) {
            Ok(task.exec(data, args_vec)?)
        } else {
            Err(Errno::ENOENT)
        }
    } else {
        Err(Errno::ENOENT)
    }
}

/// 等待子任务结束
///
/// - 参数：
///     - `pid` 接受查询子任务任务号，可选值 -1 表示任意子任务
///     - `exit_code_ptr` 目标子任务的退出码
pub fn sys_wait4(
    pid: isize,
    exit_code_ptr: *mut i32,
    _options: usize,
    _rusage: usize,
) -> SysResult<usize> {
    // TODO[ABI-COMPAT]: 当前仅实现 waitpid 子集，尚未处理 options / rusage。
    let task = current_task().expect("[kernel] current task is None.");
    let mut task_inner = task.inner_exclusive_access();

    // 无法找到目标子任务则返回
    if task_inner
        .children
        .iter()
        .find(|p| pid == -1 || pid as usize == p.pid())
        .is_none()
    {
        return Err(Errno::ECHILD);
    }
    // 得到目标子任务
    let pair = task_inner.children.iter().enumerate().find(|(_, p)| {
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.pid())
    });

    if let Some((idx, _)) = pair {
        // 子任务已经结束，应当已被回收（由 exit_current_and_run_next 实现）
        // 此时仅有其父任务拥有其原子引用
        let child = task_inner.children.remove(idx);
        assert_eq!(Arc::strong_count(&child), 1);
        let child_pid = child.pid();
        let exit_code = child.inner_exclusive_access().exit_code;
        unsafe {
            *exit_code_ptr = exit_code;
        }

        Ok(child_pid)
    } else {
        // 存在目标子任务但仍未结束
        Err(Errno::EAGAIN)
    }
}

/// 系统调用 sys-setpriority
/// TODO[UNIMPLEMENTED]: 需要补完 setpriority 逻辑。
pub fn sys_setpriority(which: usize, who: usize, prio: isize) -> SysResult<usize> {
    let _ = (which, who, prio);
    Err(Errno::ENOSYS)
}

/// 系统调用 sys-getpid
pub fn sys_getpid() -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    Ok(task.pid())
}

/// 系统调用 sys-getppid
/// TODO[UNIMPLEMENTED]: 需要补完 getppid 逻辑。
pub fn sys_getppid() -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    Ok(task.ppid())
}
