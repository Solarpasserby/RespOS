// os/src/syscall/process.rs

use super::time::TimeVal;
use super::{Errno, SysResult};
use crate::fs::{AT_FDCWD, path_open};
use crate::loader::get_app_data_by_name;
use crate::mm::{copy_cstr_from_user, copy_from_user, copy_to_user, extract_cstrings_from_user};
use crate::task::{
    CloneFlags, MAX_SIG, SignalAction, SignalFlags, TASK_MANAGER, WaitOption, add_task,
    current_task, do_futex, exit_and_run_next, exit_group_and_run_next, yield_current_task,
};
use alloc::vec::Vec;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct RUsage {
    pub ru_utime: TimeVal,
    pub ru_stime: TimeVal,
    pub ru_maxrss: isize,
    pub ru_ixrss: isize,
    pub ru_idrss: isize,
    pub ru_isrss: isize,
    pub ru_minflt: isize,
    pub ru_majflt: isize,
    pub ru_nswap: isize,
    pub ru_inblock: isize,
    pub ru_oublock: isize,
    pub ru_msgsnd: isize,
    pub ru_msgrcv: isize,
    pub ru_nsignals: isize,
    pub ru_nvcsw: isize,
    pub ru_nivcsw: isize,
}

impl Default for RUsage {
    fn default() -> Self {
        Self {
            ru_utime: TimeVal { sec: 0, usec: 0 },
            ru_stime: TimeVal { sec: 0, usec: 0 },
            ru_maxrss: 0,
            ru_ixrss: 0,
            ru_idrss: 0,
            ru_isrss: 0,
            ru_minflt: 0,
            ru_majflt: 0,
            ru_nswap: 0,
            ru_inblock: 0,
            ru_oublock: 0,
            ru_msgsnd: 0,
            ru_msgrcv: 0,
            ru_nsignals: 0,
            ru_nvcsw: 0,
            ru_nivcsw: 0,
        }
    }
}

/// 系统调用 sys_exit_group
///
/// 退出单个线程
pub fn sys_exit(exit_code: i32) -> ! {
    exit_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

/// 系统调用 sys_exit_group
///
/// 退出整个线程组
pub fn sys_exit_group(exit_code: i32) -> ! {
    exit_group_and_run_next(exit_code);
    panic!("Unreachable in sys_exit_group!");
}

pub fn sys_sched_yield() -> SysResult<usize> {
    yield_current_task();
    Ok(0)
}

pub fn sys_clone(
    flags: usize,
    stack: usize,
    ptid: usize,
    tls: usize,
    ctid: usize,
) -> SysResult<usize> {
    let flags = CloneFlags::from_bits(flags as u32).ok_or(Errno::EINVAL)?;

    // 简化模型：CLONE_THREAD 表示真正线程，必须共享地址空间。
    // 不共享地址空间的可调度实体按新进程处理，而不是放进同一线程组。
    if flags.contains(CloneFlags::CLONE_THREAD) && !flags.contains(CloneFlags::CLONE_VM) {
        return Err(Errno::EINVAL);
    }
    if flags.contains(CloneFlags::CLONE_SIGHAND) && !flags.contains(CloneFlags::CLONE_VM) {
        return Err(Errno::EINVAL);
    }

    let current_task = current_task().expect("[kernel] current task is None.");
    // 此处发生任务复制
    let new_task = current_task.clone_(flags);
    let new_tid = new_task.tid();

    // CLONE_PARENT_SETTID: 在父进程地址空间写入子进程 tid
    if flags.contains(CloneFlags::CLONE_PARENT_SETTID) && ptid != 0 {
        let tid_val = new_tid as u32;
        copy_to_user(ptid as *mut u32, &tid_val as *const u32, 1)?;
    }

    // CLONE_CHILD_SETTID: 子线程开始运行前在 ctid 写入自己的 tid。
    if flags.contains(CloneFlags::CLONE_CHILD_SETTID) && ctid != 0 {
        let tid_val = new_tid as u32;
        copy_to_user(ctid as *mut u32, &tid_val as *const u32, 1)?;
        new_task.set_set_child_tid(ctid);
    }

    // CLONE_CHILD_CLEARTID: 记录线程退出时清零并唤醒的地址
    if flags.contains(CloneFlags::CLONE_CHILD_CLEARTID) && ctid != 0 {
        new_task.set_clear_child_tid(ctid);
    }

    // 修改新任务的异常上下文，修改栈指针和返回值。
    // x4(tp) 属于用户态 TLS，不能写成内核 TaskControlBlock 指针。
    let new_task_trap_cx = new_task.get_trap_cx();
    if stack != 0 {
        new_task_trap_cx.set_sp(stack);
    }
    if flags.contains(CloneFlags::CLONE_SETTLS) {
        new_task_trap_cx.set_tp(tls);
    }
    new_task_trap_cx.set_a0(0);

    add_task(new_task);
    // 系统调用返回新创建任务的 pid
    Ok(new_tid)
}

pub fn sys_execve(path: *const u8, args: *const usize, envp: *const usize) -> SysResult<usize> {
    let path = copy_cstr_from_user(path)?;
    let args_vec = extract_cstrings_from_user(args)?;
    let envs_vec = if envp.is_null() {
        Vec::new()
    } else {
        extract_cstrings_from_user(envp)?
    };
    let task = current_task().expect("[kernel] current task is None.");

    if let Ok(file) = path_open(AT_FDCWD, &path, 0, 0) {
        info!("[kernel] execute file in fs");
        let all_data = file.read_all()?;
        Ok(task.execve(all_data.as_slice(), args_vec, envs_vec)?)
    } else if !path.starts_with("/") {
        // 从内核中加载的应用程序
        if let Some(data) = get_app_data_by_name(path.as_str()) {
            Ok(task.execve(data, args_vec, envs_vec)?)
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
///     - `exit_code_ptr` 目标子任务的 wait status
pub fn sys_wait4(
    pid: isize,
    exit_code_ptr: *mut i32,
    options: usize,
    rusage: *mut RUsage,
) -> SysResult<usize> {
    let options = WaitOption::from_bits(options as i32).ok_or(Errno::EINVAL)?;

    // pid == 0 和 pid < -1 需要按进程组等待；当前任务结构尚未维护 pgid，
    // 先显式拒绝，避免把进程组语义错误地退化成等待任意子任务。
    if pid == 0 || pid < -1 {
        return Err(Errno::EINVAL);
    }

    let nohang = options.contains(WaitOption::WNOHANG);

    loop {
        let task = current_task().expect("[kernel] current task is None.");
        let wait_result = task.op_children_mut(|children| {
            let matches_pid = |child_tid: usize| pid == -1 || pid as usize == child_tid;

            if !children.keys().any(|child_tid| matches_pid(*child_tid)) {
                return Err(Errno::ECHILD);
            }

            Ok(children
                .iter()
                .find(|(child_tid, child)| matches_pid(**child_tid) && child.is_exited())
                .map(|(child_tid, child)| (*child_tid, (child.exit_code() & 0xff) << 8)))
        })?;

        if let Some((child_tid, wait_status)) = wait_result {
            if !exit_code_ptr.is_null() {
                copy_to_user(exit_code_ptr, &wait_status as *const i32, 1)?;
            }

            // 当前内核还没有任务资源统计，先按 wait4 ABI 写回零值结构。
            if !rusage.is_null() {
                let usage = RUsage::default();
                copy_to_user(rusage, &usage as *const RUsage, 1)?;
            }

            task.op_children_mut(|children| {
                children.remove(&child_tid);
            });

            warn! {"[kernel] (wait4) parent:{}, child:{}.", task.tid(), child_tid};

            return Ok(child_tid);
        }

        if nohang {
            return Ok(0);
        }

        yield_current_task();
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
    Ok(task.op_parent(|parent| parent.as_ref().unwrap().upgrade().unwrap().tid()))
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

/// 系统调用 sys_set_tid_address
///
/// musl 初始化线程库时调用，设置 clear-child-tid 地址。
/// 与 CLONE_CHILD_CLEARTID 配合，在线程退出时向该地址写入 0 并 futex wake，
/// 以唤醒 wait4 / pthread_join 的调用者。
pub fn sys_set_tid_address(tidptr: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    task.set_clear_child_tid(tidptr);
    Ok(task.tid())
}

/// 系统调用 sys_futex - 快速用户空间互斥锁
///
/// FUTEX_WAIT: 如果 *uaddr == val ，则阻塞当前任务；否则返回 EAGAIN
/// FUTEX_WAKE: 唤醒最多 val 个阻塞在 uaddr 上的任务，返回实际唤醒数
pub fn sys_futex(
    uaddr: *const i32,
    futex_op: usize,
    val: usize,
    timeout: usize,
    uaddr2: usize,
    val3: usize,
) -> SysResult<usize> {
    do_futex(uaddr as usize, futex_op, val, timeout, uaddr2, val3)
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

        let flag = SignalFlags::from_bits(1u32 << signum).ok_or(Errno::EINVAL)?;

        if check_sigaction_error(flag, action as usize, old_action as usize) {
            return Err(Errno::EINVAL);
        }

        signal_inner.signal_actions.table[signum as usize]
    }; // 这里 signal_inner 自动释放

    // 写回旧动作（无锁）
    if !old_action.is_null() {
        copy_to_user(old_action, &prev_action as *const SignalAction, 1)?;
    }

    // 如果需要设置新动作
    if !action.is_null() {
        let mut new_action = SignalAction::default();

        // 从用户空间读取（无锁）
        copy_from_user(&mut new_action as *mut SignalAction, action, 1)?;

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
