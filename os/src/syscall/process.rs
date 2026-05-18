// os/src/syscall/process.rs
use alloc::sync::Arc;
use crate::task::{
    current_task,
    add_task,
    SignalFlags,
    SignalAction,
    MAX_SIG,
    pid2task,
    exit_current_and_run_next,
    suspend_current_and_run_next,
};
use crate::loader::get_app_data_by_name;
use crate::timer::get_time_ms;
use crate::mm::{copy_from_user, copy_to_user};
use crate::mm::copy_cstr_from_user;
use super::{SysResult, Errno};
pub fn sys_exit(exit_code: i32) -> ! {
    // println!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> SysResult<usize> {
    suspend_current_and_run_next();
    Ok(0)
}

pub fn sys_get_time() -> SysResult<usize> {
    Ok(get_time_ms())
}

fn check_sigaction_error(signal: SignalFlags, action: usize, old_action: usize) -> bool {
    if action == 0
        || old_action == 0
        || signal == SignalFlags::SIGKILL
        || signal == SignalFlags::SIGSTOP
    {
        true
    } else {
        false
    }
}

pub fn sys_fork() -> SysResult<usize> {
    let current_task = current_task().unwrap();
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

pub fn sys_exec(path: *const u8) -> SysResult<usize> {
    let path = copy_cstr_from_user(path)?;
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        Ok(0)
    } else {
        Err(Errno::ENOENT)
    }
}

/// 等待子任务结束
///
/// - 参数：
///     - `pid` 接受查询子任务任务号，可选值 -1 表示任意子任务
///     - `exit_code_ptr` 目标子任务的退出码
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> SysResult<usize> {
    let task = current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();

    // 无法找到目标子任务则返回
    if task_inner.children.iter()
        .find(|p| pid == -1 || pid as usize == p.pid())
        .is_none() {
        return Err(Errno::ECHILD);
    }
    // 得到目标子任务
    let pair = task_inner.children.iter()
        .enumerate()
        .find(|(_, p)| {
            p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.pid())
        });

    if let Some((idx, _)) = pair {
        // 子任务已经结束，应当已被回收（由 exit_current_and_run_next 实现）
        // 此时仅有其父任务拥有其原子引用
        let child = task_inner.children.remove(idx);
        assert_eq!(Arc::strong_count(&child), 1);
        let child_pid = child.pid();
        let exit_code = child.inner_exclusive_access().exit_code;
        unsafe { *exit_code_ptr = exit_code; }

        Ok(child_pid)
    } else { // 存在目标子任务但仍未结束
        Err(Errno::EAGAIN)
    }
}

pub fn sys_kill(pid: usize, signum: i32) -> SysResult<usize> {
    if let Some(task) = pid2task(pid) {
        if let Some(flag) = SignalFlags::from_bits(1 << signum) {
            let mut task_ref = task.inner_exclusive_access();
            if task_ref.signals.contains(flag) {
                // 信号已存在，返回错误
                return Err(Errno::EINVAL);
            }
            task_ref.signals.insert(flag);
            Ok(0) // 成功返回 Ok(0)
        } else {
            // 信号不合法
            Err(Errno::EINVAL)
        }
    } else {
        // 进程不存在
        Err(Errno::ESRCH)
    }
}

pub fn sys_sigaction(
    signum: i32,
    action: *const SignalAction,
    old_action: *mut SignalAction,
) -> SysResult<usize> {
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();

    // 检查信号号是否越界，非法参数 → 返回 EINVAL
    if signum as usize > MAX_SIG {
        return Err(Errno::EINVAL);
    }

    if let Some(flag) = SignalFlags::from_bits(1 << signum) {
        if check_sigaction_error(flag, action as usize, old_action as usize) {
            // 检查失败 → 返回 EINVAL
            return Err(Errno::EINVAL);
        }

        let prev_action = inner.signal_actions.table[signum as usize];

        // 将旧规则拷贝到用户空间
        copy_to_user(
            old_action,
            &prev_action as *const SignalAction,
            1,
        )?;

        // 从用户空间读取新规则
        let mut new_action = SignalAction::default();
        copy_from_user(
            &mut new_action as *mut SignalAction,
            action,
            1,
        )?;

        // 更新当前信号的处理规则
        inner.signal_actions.table[signum as usize] = new_action;

        // 成功返回 Ok(0)
        Ok(0)
    } else {
        // 信号无效 → 返回 EINVAL
        Err(Errno::EINVAL)
    }
}

pub fn sys_sigprocmask(mask: u32) -> SysResult<usize> {
    if let Some(task) = current_task() {
        let mut inner = task.inner_exclusive_access();
        let old_mask = inner.signal_mask;
        if let Some(flag) = SignalFlags::from_bits(mask) {
            inner.signal_mask = flag;
            Ok(old_mask.bits() as usize)
        } else {
            Err(Errno::EINVAL)
        }
    } else {
        Err(Errno::ESRCH)
    }
}

pub fn sys_sigreturn() -> SysResult<usize> {
    if let Some(task) = current_task() {
        let mut inner = task.inner_exclusive_access();
        inner.handling_sig = -1;
        let trap_ctx = task.get_trap_cx();
        *trap_ctx = inner.trap_ctx_backup.take().unwrap();
        Ok(trap_ctx.x[10] as usize)
    } else {
        Err(Errno::ESRCH)
    }
}