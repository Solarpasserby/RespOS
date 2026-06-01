use super::{Errno, SysResult};
use crate::mm::{copy_from_user, copy_to_user};
use crate::signal::sig_handler::SigAction;
use crate::signal::sig_stack::SigContext;
use crate::signal::{SiField, Sig, SigInfo, SigSet};
use crate::task::{TASK_MANAGER, current_task};

pub fn sys_kill(pid: usize, signum: i32) -> SysResult<usize> {
    //POSIX 规定：signum 为 0 时不发任何信号，只检查"pid 是否存在 + 我有没有权限发"。权限检查先跳过，存在性检查后面自然覆盖——如果 pid不存在下面会返回 ESRCH。
    if signum == 0 {
        return Ok(0);
    }
    let sig = Sig::from(signum);
    if !sig.is_valid() {
        return Err(Errno::EINVAL);
    }
    if let Some(task) = TASK_MANAGER.get(pid) {
        let siginfo = SigInfo::new(
            sig.raw(),
            SigInfo::USER,
            SiField::Kill {
                tid: current_task().unwrap().tgid(), // 获取发送者的进程号
            },
        );
        // 主线程 → 进程级信号（整个线程组）；普通线程 → 线程级信号
        task.receive_siginfo(siginfo, !task.is_process_leader());
        Ok(0)
    } else {
        Err(Errno::ESRCH)
    }
}

pub fn sys_tkill(tid: usize, signum: i32) -> SysResult<usize> {
    if signum == 0 {
        return Ok(0);
    }
    let sig = Sig::from(signum);
    if !sig.is_valid() {
        return Err(Errno::EINVAL);
    }
    if let Some(task) = TASK_MANAGER.get(tid) {
        let siginfo = SigInfo::new(
            sig.raw(),
            SigInfo::TKILL,
            SiField::Kill {
                tid: current_task().unwrap().tid(), //获取发送者的线程号
            },
        );
        task.receive_siginfo(siginfo, true);
        Ok(0)
    } else {
        Err(Errno::ESRCH)
    }
}

pub fn sys_sigaction(signum: i32, act: *const u8, oldact: *mut u8) -> SysResult<usize> {
    if signum <= 0 || signum > 64 {
        return Err(Errno::EINVAL);
    }
    let sig = Sig::from(signum);
    if sig.is_kill_or_stop() {
        return Err(Errno::EINVAL);
    }

    let act_ptr = act as *const SigAction;
    let oldact_ptr = oldact as *mut SigAction;
    let task = current_task().expect("[kernel] current task is None.");

    // 写回旧动作
    if !oldact.is_null() {
        let old_action = task.op_sig_handler(|handler| handler.get(sig));
        copy_to_user(oldact_ptr, &old_action as *const SigAction, 1)?;
    }

    // 读入新动作
    if !act.is_null() {
        let mut new_action: SigAction = unsafe { core::mem::zeroed() }; // 初始化，不用default是因为字段 SigActionFlag 和 SigSet 的 bitflags 版本（1.2.1）不自动实现 Default
        copy_from_user(&mut new_action as *mut SigAction, act_ptr, 1)?;
        new_action.mask.remove_signal(Sig::SIGKILL);
        new_action.mask.remove_signal(Sig::SIGSTOP);
        task.op_sig_handler_mut(|handler| handler.update(sig, new_action));
    }

    Ok(0)
}

pub fn sys_sigprocmask(
    how: usize,
    set: usize,
    oldset: usize,
    sigsetsize: usize,
) -> SysResult<usize> {
    const SIG_BLOCK: usize = 0;
    const SIG_UNBLOCK: usize = 1;
    const SIG_SETMASK: usize = 2;

    if how > SIG_SETMASK {
        return Err(Errno::EINVAL);
    }
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return Err(Errno::EINVAL);
    }

    let set_ptr = set as *const SigSet;
    let oldset_ptr = oldset as *mut SigSet;
    let task = current_task().expect("[kernel] current task is None.");

    let current_mask = task.op_sig_pending(|pending| pending.mask);

    // 写回旧掩码
    if oldset != 0 {
        copy_to_user(oldset_ptr, &current_mask as *const SigSet, 1)?;
    }

    // 读入新掩码并计算
    if set != 0 {
        // set 为 NULL → 不修改，只查询当前掩码写入 oldset。
        let mut new_mask: SigSet = unsafe { core::mem::zeroed() };
        copy_from_user(&mut new_mask as *mut SigSet, set_ptr, 1)?;

        // SIGKILL 和 SIGSTOP 不可被屏蔽
        new_mask.remove_signal(Sig::SIGKILL);
        new_mask.remove_signal(Sig::SIGSTOP);

        let new_mask = match how {
            SIG_BLOCK => current_mask | new_mask,
            SIG_UNBLOCK => current_mask & !new_mask,
            SIG_SETMASK => new_mask,
            _ => unreachable!(),
        };

        task.op_sig_pending_mut(|pending| {
            pending.change_mask(new_mask);
        });
    }

    Ok(0)
}

pub fn sys_sigreturn() -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");

    // 获取当前 trapframe（在 kernel stack 上）
    let trap_cx = task.get_trap_cx();

    // 从用户栈顶读取 SigContext
    let sig_context_addr = task.sig_context_addr();
    if sig_context_addr == 0 {
        return Err(Errno::EFAULT); // 没注册过 handler 就调 sigreturn，拒绝
    }
    let sig_context_ptr = sig_context_addr as *const SigContext;
    let mut sig_context: SigContext = unsafe { core::mem::zeroed() };
    copy_from_user(&mut sig_context as *mut SigContext, sig_context_ptr, 1)?;

    // 普通 handler（info == 0）：恢复寄存器和 sepc
    if sig_context.info == 0 {
        trap_cx.x = sig_context.x;
        trap_cx.sepc = sig_context.sepc;
    }
    // TODO : info == 1，SA_SIGINFO 路径。
    // 恢复信号掩码
    task.op_sig_pending_mut(|pending| {
        pending.mask = sig_context.mask;
    });

    Ok(trap_cx.x[10] as usize) // a0 作为返回值
}
