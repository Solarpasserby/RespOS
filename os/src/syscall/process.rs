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
use crate::task::PID2TCB;


pub fn sys_exit(exit_code: i32) -> ! {
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

    // 得到已经退出（Zombie）的目标子任务
    let pair = task_inner.children.iter()
        .enumerate()
        .find(|(_, p)| {
            p.inner_exclusive_access().is_zombie()
                && (pid == -1 || pid as usize == p.pid())
        });

    if let Some((idx, _)) = pair {
        // 从 children 中移除
        let child = task_inner.children.remove(idx);;
        // 此时只剩父进程持有这一份 Arc
        assert_eq!(Arc::strong_count(&child), 1);

        let child_pid = child.pid();
        let exit_code = child.inner_exclusive_access().exit_code;

        // 写回退出码（如果指针非空）
        if !exit_code_ptr.is_null() {
            unsafe {
                *exit_code_ptr = exit_code;
            }
        }

        //在真正回收子进程时，从 PID2TCB 中删除
        { &*PID2TCB }
            .lock()
            .remove(&child_pid);

        Ok(child_pid)
    } else {
        // 存在目标子任务，但尚未退出
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
    // 先检查信号编号是否合法
    if signum < 0 || signum as usize >= MAX_SIG {
        return Err(Errno::EINVAL);
    }

    // 先获取旧动作（持锁）
    let prev_action = {
        let task = current_task().unwrap();
        let inner = task.inner_exclusive_access();

        let flag = SignalFlags::from_bits(1u32 << signum)
            .ok_or(Errno::EINVAL)?;

        if check_sigaction_error(flag, action as usize, old_action as usize) {
            return Err(Errno::EINVAL);
        }

        inner.signal_actions.table[signum as usize]
    }; // ← 这里 inner 自动释放

    // 写回旧动作（无锁）
    if !old_action.is_null() {
        copy_to_user(
            old_action,
            &prev_action as *const SignalAction,
            1,
        )?;
    }

    // 如果需要设置新动作
    if !action.is_null() {
        let mut new_action = SignalAction::default();

        // 从用户空间读取（无锁）
        copy_from_user(
            &mut new_action as *mut SignalAction,
            action,
            1,
        )?;

        // 再次加锁并更新表
        let task = current_task().unwrap();
        let mut inner = task.inner_exclusive_access();
        inner.signal_actions.table[signum as usize] = new_action;
    }

    Ok(0)
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