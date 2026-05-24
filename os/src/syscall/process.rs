// os/src/syscall/process.rs

use super::{SysResult, Errno};
use crate::task::{
    MAX_SIG,
    TASK_MANAGER,
    CloneFlags,
    SignalFlags,
    SignalAction,
    current_task,
    yield_current_task,
    exit_and_run_next,
};
use crate::mm::{copy_cstr_from_user, copy_to_user, copy_from_user, extract_cstrings_from_user};
use crate::fs::{AT_FDCWD, path_open};
use crate::loader::get_app_data_by_name;

pub fn sys_exit(exit_code: i32) -> ! {
    exit_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_sched_yield() -> SysResult<usize> {
    yield_current_task();
    Ok(0)
}

pub fn sys_clone(
    flags: usize,
    _stack: usize,
    _ptid: usize,
    _tls: usize,
    _ctid: usize,
) -> SysResult<usize> {
    // TODO[ABI-COMPAT]: 当前仅复用了 fork 语义，尚未真正支持 clone 的 flags/stack/tls 等能力。
    let current_task = current_task().expect("[kernel] current task is None.");
    // 此处发生任务复制
    let new_task = current_task.fork(CloneFlags::from_bits_truncate(flags as u32));
    let new_tid = new_task.tid();
    // 修改新任务的异常上下文，将其 sys_fork 的返回值设为 0
    let new_task_cx = new_task.get_trap_cx();
    new_task_cx.x[10] = 0;
    // 系统调用返回新创建任务的 pid
    Ok(new_tid)
}

pub fn sys_execve(path: *const u8, args: *const usize, _envp: *const usize) -> SysResult<usize> {
    // TODO[ABI-COMPAT]: 当前忽略 envp，后续如需完整 execve 语义应补上环境变量处理。
    let path = copy_cstr_from_user(path)?;
    let args_vec = extract_cstrings_from_user(args)?;
    let task = current_task().expect("[kernel] current task is None.");

    if let Ok(file) = path_open(AT_FDCWD, &path, 0, 0) {
        info!("[kernel] execute file in fs");
        let all_data = file.read_all()?;
        Ok(task.execve(all_data.as_slice(), args_vec)?)
    } else if !path.starts_with("/") {
        // 从内核中加载的应用程序
        if let Some(data) = get_app_data_by_name(path.as_str()) {
            Ok(task.execve(data, args_vec)?)
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

    let child = task.op_children_mut(|children| {
        let matches_pid = |child_tid: usize| pid == -1 || pid as usize == child_tid;

        if !children.keys().any(|child_tid| matches_pid(*child_tid)) {
            return Err(Errno::ECHILD);
        }

        let exited_tid = children
            .iter()
            .find(|(child_tid, child)| matches_pid(**child_tid) && child.is_exited())
            .map(|(child_tid, _)| *child_tid);

        match exited_tid {
            Some(child_tid) => Ok(Some(children.remove(&child_tid).unwrap())),
            None => Ok(None),
        }
    })?;

    if let Some(child) = child {
        let child_tid = child.tid();
        let exit_code = child.exit_code();

        // 写回退出码（如果指针非空）
        if !exit_code_ptr.is_null() {
            unsafe {
                *exit_code_ptr = exit_code;
            }
        }

        Ok(child_tid)
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
    Ok(task.tid())
}

/// 系统调用 sys-getppid
pub fn sys_getppid() -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    Ok(task
        .op_parent(|parent| {
            parent.as_ref().unwrap().upgrade().unwrap().tid()
        })
    )
}

pub fn sys_kill(pid: usize, signum: i32) -> SysResult<usize> {
    if let Some(task) = TASK_MANAGER.get(pid) {
        if let Some(flag) = SignalFlags::from_bits(1 << signum) {
            let mut task_ref = task.get_signal_inner();
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
        let signal_inner = task.get_signal_inner();

        let flag = SignalFlags::from_bits(1u32 << signum)
            .ok_or(Errno::EINVAL)?;

        if check_sigaction_error(flag, action as usize, old_action as usize) {
            return Err(Errno::EINVAL);
        }

        signal_inner.signal_actions.table[signum as usize]
    }; // 这里 signal_inner 自动释放

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
        let mut signal_inner = task.get_signal_inner();
        signal_inner.signal_actions.table[signum as usize] = new_action;
    }

    Ok(0)
}

pub fn sys_sigprocmask(mask: u32) -> SysResult<usize> {
    if let Some(task) = current_task() {
        let mut signal_inner = task.get_signal_inner();
        let old_mask = signal_inner.signal_mask;
        if let Some(flag) = SignalFlags::from_bits(mask) {
            signal_inner.signal_mask = flag;
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
        let mut signal_inner = task.get_signal_inner();
        signal_inner.handling_sig = -1;
        let trap_ctx = task.get_trap_cx();
        *trap_ctx = signal_inner.trap_ctx_backup.take().unwrap();
        Ok(trap_ctx.x[10] as usize)
    } else {
        Err(Errno::ESRCH)
    }
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