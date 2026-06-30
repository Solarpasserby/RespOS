use super::{Errno, SysResult};
use crate::mm::{copy_from_user, copy_to_user};
use crate::signal::sig_handler::SigAction;
use crate::signal::sig_stack::{SS_DISABLE, SignalStack};
use crate::signal::sig_struct::{FrameFlags, Sig, SigFrame, SigRTFrame, SigSet};
use crate::signal::{LinuxSigInfo, SiField, SigInfo};
use crate::task::{TASK_MANAGER, current_task, yield_current_task};
use crate::timer::{TimeSpec, get_timeout_ms};
use alloc::vec::Vec;

#[cfg(target_arch = "loongarch64")]
#[derive(Clone, Copy)]
#[repr(C)]
struct UserSigAction {
    handler: usize,
    flags: usize,
    restorer: usize,
    mask: SigSet,
}

#[cfg(not(target_arch = "loongarch64"))]
type UserSigAction = SigAction;

#[cfg(target_arch = "loongarch64")]
fn sigaction_from_user(action: UserSigAction) -> SigAction {
    SigAction {
        sa_handler: action.handler,
        flags: crate::signal::sig_handler::SigActionFlag::from_bits_truncate(action.flags as u32),
        restorer: action.restorer,
        mask: action.mask,
    }
}

#[cfg(not(target_arch = "loongarch64"))]
fn sigaction_from_user(action: UserSigAction) -> SigAction {
    action
}

#[cfg(target_arch = "loongarch64")]
fn sigaction_to_user(action: SigAction) -> UserSigAction {
    UserSigAction {
        handler: action.sa_handler,
        flags: action.flags.bits() as usize,
        restorer: action.restorer,
        mask: action.mask,
    }
}

#[cfg(not(target_arch = "loongarch64"))]
fn sigaction_to_user(action: SigAction) -> UserSigAction {
    action
}

#[cfg(target_arch = "riscv64")]
fn restore_sig_context(
    trap_cx: &mut crate::arch::trap::TrapContext,
    ctx: crate::signal::sig_stack::SigContext,
) {
    trap_cx.x[0] = 0;
    trap_cx.x[1..].copy_from_slice(&ctx.gregs[1..]);
    trap_cx.set_sepc(ctx.gregs[0]);
}

#[cfg(target_arch = "loongarch64")]
fn restore_sig_context(
    trap_cx: &mut crate::arch::trap::TrapContext,
    ctx: crate::signal::sig_stack::SigContext,
) {
    trap_cx.x = ctx.gregs;
    trap_cx.set_sepc(ctx.pc);
}

pub fn sys_kill(pid: usize, signum: i32) -> SysResult<usize> {
    let sig = Sig::from(signum);
    if signum != 0 && !sig.is_valid() {
        return Err(Errno::EINVAL);
    }

    let current = current_task().expect("[kernel] current task is None.");
    let pid = pid as isize;
    let mut targets = Vec::new();
    if pid > 0 {
        if let Some(task) = TASK_MANAGER.get(pid as usize) {
            targets.push(task);
        }
    } else {
        let pgid = if pid == 0 {
            current.pgid()
        } else if pid == -1 {
            usize::MAX
        } else {
            (-pid) as usize
        };
        TASK_MANAGER.for_each(|task| {
            if task.tid() == task.tgid()
                && (pgid == usize::MAX || task.pgid() == pgid)
                && !(pid == -1 && task.tgid() == current.tgid())
            {
                targets.push(task.clone());
            }
        });
    }

    if targets.is_empty() {
        return Err(Errno::ESRCH);
    }
    if signum == 0 {
        return Ok(0);
    }

    let mut delivered = false;
    let mut denied = false;
    for task in targets {
        if current.euid() != 0
            && current.euid() != task.uid()
            && current.euid() != task.suid()
            && current.uid() != task.uid()
            && current.uid() != task.suid()
        {
            denied = true;
            continue;
        }
        let siginfo = SigInfo::new(
            sig.raw(),
            SigInfo::USER,
            SiField::Kill {
                tid: current.tgid(),
            },
        );
        task.receive_siginfo(siginfo, !task.is_process_leader());
        if sig == Sig::SIGCONT && task.is_stopped() {
            task.set_wait_event(SigInfo::CLD_CONTINUED, sig.raw());
            task.notify_parent_sigchld(SigInfo::CLD_CONTINUED);
            crate::task::wakeup_stopped_task(task);
        }
        delivered = true;
    }
    if delivered {
        Ok(0)
    } else if denied {
        Err(Errno::EPERM)
    } else {
        Err(Errno::ESRCH)
    }
}

pub fn sys_tkill(tid: usize, signum: i32) -> SysResult<usize> {
    if (tid as isize) <= 0 {
        return Err(Errno::EINVAL);
    }
    let sig = Sig::from(signum);
    if signum != 0 && !sig.is_valid() {
        return Err(Errno::EINVAL);
    }
    if let Some(task) = TASK_MANAGER.get(tid) {
        if signum == 0 {
            return Ok(0);
        }
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

pub fn sys_tgkill(tgid: usize, tid: usize, signum: i32) -> SysResult<usize> {
    if (tgid as isize) <= 0 || (tid as isize) <= 0 {
        return Err(Errno::EINVAL);
    }
    if let Some(task) = TASK_MANAGER.get(tid) {
        if tgid != 0 && task.tgid() != tgid {
            return Err(Errno::ESRCH);
        }
        if signum == 0 {
            return Ok(0);
        }
        if signum > Sig::SIGLEGACYMAX.raw()
            && task.op_sig_pending(|pending| pending.mask.contain_signal(Sig::from(signum)))
        {
            return Err(Errno::EAGAIN);
        }
    } else {
        return Err(Errno::ESRCH);
    }
    sys_tkill(tid, signum)
}

pub fn sys_sigaltstack(
    new_stack: *const SignalStack,
    old_stack: *mut SignalStack,
) -> SysResult<usize> {
    const SS_ONSTACK: i32 = 2;
    const MINSIGSTKSZ: usize = 2048;

    let task = current_task().expect("[kernel] current task is None.");
    if !old_stack.is_null() {
        let old = task.raw_sigstack();
        copy_to_user(old_stack, &old as *const SignalStack, 1)?;
    }
    if !new_stack.is_null() {
        let mut stack = SignalStack::default();
        copy_from_user(&mut stack as *mut SignalStack, new_stack, 1)?;
        if stack.ss_flags & !(SS_DISABLE as i32) != 0 {
            return Err(Errno::EINVAL);
        }
        if stack.ss_flags & SS_ONSTACK != 0 {
            return Err(Errno::EINVAL);
        }
        if stack.ss_flags & (SS_DISABLE as i32) == 0 && stack.ss_size < MINSIGSTKSZ {
            return Err(Errno::ENOMEM);
        }
        task.set_sigstack(stack);
    }
    Ok(0)
}

pub fn sys_rt_sigpending(set: *mut SigSet, sigsetsize: usize) -> SysResult<usize> {
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return Err(Errno::EINVAL);
    }
    let task = current_task().expect("[kernel] current task is None.");
    let pending = task.op_sig_pending(|pending| pending.pending);
    copy_to_user(set, &pending as *const SigSet, 1)?;
    Ok(0)
}

pub fn sys_rt_sigsuspend(mask: *const SigSet, sigsetsize: usize) -> SysResult<usize> {
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return Err(Errno::EINVAL);
    }

    let mut new_mask = SigSet::empty();
    copy_from_user(&mut new_mask as *mut SigSet, mask, 1)?;
    new_mask.remove_signal(Sig::SIGKILL);
    new_mask.remove_signal(Sig::SIGSTOP);

    let task = current_task().expect("[kernel] current task is None.");
    let old_mask = task.op_sig_pending(|pending| pending.mask);
    task.set_sigsuspend_saved_mask(Some(old_mask));
    task.op_sig_pending_mut(|pending| pending.mask = new_mask);

    loop {
        task.set_interruptible(true);
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            break;
        }
        task.check_real_timer();
        yield_current_task();
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            break;
        }
    }

    task.set_interruptible(false);
    Err(Errno::EINTR)
}

pub fn sys_rt_sigqueueinfo(
    tgid: usize,
    signum: i32,
    uinfo: *const LinuxSigInfo,
) -> SysResult<usize> {
    let sig = Sig::from(signum);
    if signum != 0 && !sig.is_valid() {
        return Err(Errno::EINVAL);
    }
    let task = TASK_MANAGER.get(tgid).ok_or(Errno::ESRCH)?;
    let mut linux_info = LinuxSigInfo::default();
    copy_from_user(&mut linux_info as *mut LinuxSigInfo, uinfo, 1)?;
    let mut siginfo = SigInfo::from(linux_info);
    siginfo.signo = signum;
    task.receive_siginfo(siginfo, !task.is_process_leader());
    Ok(0)
}

pub fn sys_sigaction(signum: i32, act: *const u8, oldact: *mut u8) -> SysResult<usize> {
    if signum <= 0 || signum > 64 {
        return Err(Errno::EINVAL);
    }
    let sig = Sig::from(signum);
    if sig.is_kill_or_stop() {
        return Err(Errno::EINVAL);
    }

    let act_ptr = act as *const UserSigAction;
    let oldact_ptr = oldact as *mut UserSigAction;
    let task = current_task().expect("[kernel] current task is None.");

    // 写回旧动作
    if !oldact.is_null() {
        let old_action = task.op_sig_handler(|handler| handler.get(sig));
        let old_user_action = sigaction_to_user(old_action);
        copy_to_user(oldact_ptr, &old_user_action as *const UserSigAction, 1)?;
    }

    // 读入新动作
    if !act.is_null() {
        let mut new_user_action: UserSigAction = unsafe { core::mem::zeroed() };
        copy_from_user(&mut new_user_action as *mut UserSigAction, act_ptr, 1)?;
        let mut new_action = sigaction_from_user(new_user_action);
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
    let task = current_task().unwrap();
    let trap_cx = task.get_trap_cx();

    let sp = trap_cx.get_sp();
    let flag_ptr = sp as *const FrameFlags;
    let mut flag: FrameFlags = FrameFlags::default();
    copy_from_user(&mut flag as *mut FrameFlags, flag_ptr, 1)?;

    let restored_mask = if flag.is_rt() {
        // RT 帧：读 SigRTFrame
        let frame_ptr = sp as *const SigRTFrame;
        let mut frame: SigRTFrame = unsafe { core::mem::zeroed() };
        copy_from_user(&mut frame, frame_ptr, 1)?;
        let ctx = frame.ucontext.uc_mcontext;
        restore_sig_context(trap_cx, ctx);
        frame.ucontext.uc_sigmask
    } else {
        // 普通帧：读 SigFrame
        let frame_ptr = sp as *const SigFrame;
        let mut frame: SigFrame = unsafe { core::mem::zeroed() };
        copy_from_user(&mut frame, frame_ptr, 1)?;
        let ctx = frame.sigcontext;
        let restored_mask = ctx.mask;
        restore_sig_context(trap_cx, ctx);
        restored_mask
    };

    // 恢复信号掩码
    task.op_sig_pending_mut(|pending| pending.mask = restored_mask);

    Ok(trap_cx.get_a0())
}
// pub fn sys_sigreturn() -> SysResult<usize> {
//     let task = current_task().expect("[kernel] current task is None.");

//     // 获取当前 trapframe（在 kernel stack 上）
//     let trap_cx = task.get_trap_cx();

//     // 从用户栈顶读取 SigContext
//     let sig_context_addr = task.sig_context_addr();
//     if sig_context_addr == 0 {
//         return Err(Errno::EFAULT); // 没注册过 handler 就调 sigreturn，拒绝
//     }
//     let sig_context_ptr = sig_context_addr as *const SigContext;
//     let mut sig_context: SigContext = unsafe { core::mem::zeroed() };
//     copy_from_user(&mut sig_context as *mut SigContext, sig_context_ptr, 1)?;

//     // 普通 handler（info == 0）：恢复寄存器和 sepc
//     if sig_context.info == 0 {
//         trap_cx.x = sig_context.x;
//         trap_cx.sepc = sig_context.sepc;
//     }
//     // TODO : info == 1，SA_SIGINFO 路径。
//     // 恢复信号掩码
//     task.op_sig_pending_mut(|pending| {
//         pending.mask = sig_context.mask;
//     });

//     Ok(trap_cx.x[10] as usize) // a0 作为返回值
// }

fn take_sigtimedwait_signal(
    wanted_set: SigSet,
    info_ptr: usize,
    task: &crate::task::TaskControlBlock,
) -> SysResult<Option<usize>> {
    let Some((sig, siginfo)) = task.op_sig_pending(|pending| {
        pending
            .find_signal_in_set(wanted_set)
            .and_then(|sig| pending.get_info(sig).copied().map(|info| (sig, info)))
    }) else {
        return Ok(None);
    };

    // Validate and write the userspace result before consuming the signal.
    // On EFAULT Linux leaves the pending signal available for a later wait.
    if info_ptr != 0 {
        let user_siginfo: LinuxSigInfo = siginfo.into();
        copy_to_user(
            info_ptr as *mut LinuxSigInfo,
            &user_siginfo as *const LinuxSigInfo,
            1,
        )?;
    }
    task.op_sig_pending_mut(|pending| pending.fetch_signal_from_set(wanted_set));
    task.clear_interrupted();
    Ok(Some(sig.raw() as usize))
}

/// sigtimedwait: 等待 set 中的某个信号，带超时
/// 1. 读入目标信号集
/// 2. 检查是否有已挂起的信号 → 有则立即返回
/// 3. 挂起等待（轮询方式，直到信号到达或超时）
pub fn sys_rt_sigtimedwait(
    set_ptr: usize,     // 等待的信号集合的指针
    info_ptr: usize,    // 收到信号后把收到的信号的详细信息放在这里
    timeout_ptr: usize, // 最多可以等待的时间
    sigsetsize: usize,
) -> SysResult<usize> {
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return Err(Errno::EINVAL);
    }
    // ----- 1. 从用户态读入目标信号集 -----
    let mut wanted_set = SigSet::empty();
    copy_from_user(&mut wanted_set as *mut SigSet, set_ptr as *const SigSet, 1)?;
    // SIGKILL 和 SIGSTOP 不可被 sigtimedwait 等待
    wanted_set.remove_signal(Sig::SIGKILL);
    wanted_set.remove_signal(Sig::SIGSTOP);

    let task = current_task().expect("[kernel] current task is None.");

    info!(
        "[sys_rt_sigtimedwait] wanted_set: {:?}, timeout_ptr: {:#x}",
        wanted_set, timeout_ptr
    );

    // ----- 2. 检查是否已有挂起的感兴趣信号 -----
    // rt_sigtimedwait 消费 set 中的 pending signal；调用方通常已经用
    // sigprocmask 阻塞这些信号。这里不能临时解屏蔽 wanted_set，否则
    // 普通信号派发可能先调用 handler 并消费掉待等待的信号。
    if let Some(sig) = take_sigtimedwait_signal(wanted_set, info_ptr, &task)? {
        info!("[sys_rt_sigtimedwait] immediate return signal: {}", sig);
        return Ok(sig);
    }

    // ----- 3. 需要等待 -----
    task.set_interruptible(true);
    if timeout_ptr != 0 {
        // 3a. 有限等待
        let mut timeout = TimeSpec::default();
        let timeout_result = copy_from_user(
            &mut timeout as *mut TimeSpec,
            timeout_ptr as *const TimeSpec,
            1,
        );
        if let Err(err) = timeout_result {
            task.set_interruptible(false);
            return Err(err);
        }
        let total_ms = match timeout.checked_duration_ms() {
            Some(total_ms) => total_ms,
            None => {
                task.set_interruptible(false);
                return Err(Errno::EINVAL);
            }
        };

        // timeout == 0 → 立即轮询返回 EAGAIN
        if timeout.is_zero() {
            task.set_interruptible(false);
            return Err(Errno::EAGAIN);
        }

        let start_ms = get_timeout_ms();

        loop {
            // 检查信号
            match take_sigtimedwait_signal(wanted_set, info_ptr, &task) {
                Ok(Some(sig)) => {
                    info!("[sys_rt_sigtimedwait] received signal: {}", sig);
                    task.set_interruptible(false);
                    return Ok(sig);
                }
                Ok(None) => {}
                Err(err) => {
                    task.set_interruptible(false);
                    return Err(err);
                }
            }

            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }

            // 检查超时
            if get_timeout_ms().saturating_sub(start_ms) >= total_ms {
                info!("[sys_rt_sigtimedwait] timeout");
                task.set_interruptible(false);
                return Err(Errno::EAGAIN);
            }

            task.check_real_timer();
            yield_current_task();
        }
    } else {
        // 3b. 无限等待
        info!("[sys_rt_sigtimedwait] waiting indefinitely");
        loop {
            match take_sigtimedwait_signal(wanted_set, info_ptr, &task) {
                Ok(Some(sig)) => {
                    info!("[sys_rt_sigtimedwait] received signal: {}", sig);
                    task.set_interruptible(false);
                    return Ok(sig);
                }
                Ok(None) => {}
                Err(err) => {
                    task.set_interruptible(false);
                    return Err(err);
                }
            }

            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }

            task.check_real_timer();
            yield_current_task();
        }
    }
}
