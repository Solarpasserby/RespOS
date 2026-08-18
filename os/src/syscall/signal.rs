//! signal 相关系统调用的 Linux UAPI 适配层。
//!
//! 本模块负责 sigaction/sigprocmask、pending/wait、altstack、queueinfo 和 sigreturn
//! 的用户结构复制；signal 的排队、选择、frame 构造与默认动作由 `crate::signal` 和
//! task 状态共同完成。
//!
//! signal ABI 对架构布局、mask 大小和错误提交顺序非常敏感。sigreturn 读取的用户 frame
//! 完全不可信，恢复 PC/SP/状态寄存器前必须经过边界校验；等待类接口 copyout 失败时不得
//! 丢弃 pending signal。阻塞 mask 的临时替换、waiter 注册和任务睡眠必须构成单一协议，
//! 避免 signal 已入队但任务永久睡眠，或迟到的 interrupted 标记污染下一次 syscall。

use super::{Errno, SysResult};
use crate::mm::{copy_from_user, copy_to_user};
use crate::signal::sig_handler::SigAction;
use crate::signal::sig_stack::{SS_DISABLE, SignalStack};
use crate::signal::sig_struct::{FrameFlags, Sig, SigFrame, SigRTFrame, SigSet};
use crate::signal::{LinuxSigInfo, SiField, SigInfo};
use crate::task::{
    PROCESS_MANAGER, TASK_MANAGER, current_task, prepare_current_task_blocked, remove_task,
    switch_to_next_task, yield_current_task,
};
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

/// 按 kill(2) 的 pid 选择规则向单进程、进程组或可见进程集合发送信号。
///
/// 先验证信号编号和发送权限，再选择稳定 ProcessState；信号 0 只探测存在性/权限而不入队。
/// 对仍保有 pid 的 zombie 按 Linux 返回成功，即使没有存活线程可接收。进程定向信号进入
/// 进程级 pending 队列，具体 TCB 只作为唤醒提示，线程组首领退出不能使信号丢失。
pub fn sys_kill(pid: usize, signum: i32) -> SysResult<usize> {
    let sig = Sig::from(signum);
    if signum != 0 && !sig.is_valid() {
        return Err(Errno::EINVAL);
    }

    let current = current_task().expect("[kernel] current task is None.");
    let pid = pid as isize;
    let mut targets = Vec::new();
    if pid > 0 {
        if let Some(process) = PROCESS_MANAGER.get(pid as usize) {
            targets.push(process);
        }
    } else {
        let pgid = if pid == 0 {
            current.pgid()
        } else if pid == -1 {
            usize::MAX
        } else {
            (-pid) as usize
        };
        PROCESS_MANAGER.for_each(|process| {
            if (pgid == usize::MAX || process.pgid() == pgid)
                && !(pid == -1 && process.tgid() == current.tgid())
            {
                targets.push(process.clone());
            }
        });
    }

    if targets.is_empty() {
        return Err(Errno::ESRCH);
    }
    let mut delivered = false;
    let mut denied = false;
    for process in targets {
        if current.euid() != 0
            && current.euid() != process.uid()
            && current.euid() != process.suid()
            && current.uid() != process.uid()
            && current.uid() != process.suid()
        {
            denied = true;
            continue;
        }
        if signum != 0 {
            if let Some(task) = process.signal_target() {
                let siginfo = SigInfo::new(
                    sig.raw(),
                    SigInfo::USER,
                    SiField::Kill {
                        tid: current.tgid(),
                    },
                );
                task.receive_siginfo(siginfo, false);
                if sig == Sig::SIGCONT && task.is_stopped() {
                    task.set_wait_event(SigInfo::CLD_CONTINUED, sig.raw());
                    task.notify_parent_sigchld(SigInfo::CLD_CONTINUED);
                    crate::task::wakeup_stopped_task(task);
                }
            }
        }
        // 对仍保有 PID 的僵尸进程，Linux 会报告成功，尽管已无存活成员能够观察该信号。
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
        let limit = task
            .rlimit(crate::task::RLIMIT_SIGPENDING)
            .map(|(cur, _)| cur)
            .unwrap_or(usize::MAX);
        if task.try_receive_siginfo(siginfo, true, limit) {
            Ok(0)
        } else {
            Err(Errno::EAGAIN)
        }
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
    // Linux 暴露“因被阻塞而保持 pending”的信号。未阻塞信号可立即投递，
    // 不能仅因它尚处于入队与下次返回用户态检查之间就将其报告为 pending。
    let pending = task.pending_blocked_set();
    copy_to_user(set, &pending as *const SigSet, 1)?;
    Ok(0)
}

/// 临时替换线程信号掩码并睡眠，直到一个可递送信号中断等待。
///
/// 新 mask 先 copyin 并移除 SIGKILL/SIGSTOP，原 mask 保存到任务专用槽位，由下一次信号帧
/// 记录并在 sigreturn 恢复。等待使用“发布 interruptible/Blocked → 复查 pending”的顺序，
/// 避免信号到达后仍睡眠；正常语义总以 EINTR 离开，不能提前恢复 mask 而让 handler
/// 观察到错误屏蔽集。
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
        if prepare_current_task_blocked() {
            // 关闭信号在发布 Blocked 前到达所造成的丢失唤醒窗口。
            if task.is_ready() || task.check_signal_interrupt() || task.is_interrupted() {
                remove_task(task.tid());
                task.set_running();
            } else {
                switch_to_next_task();
            }
        } else {
            crate::perf::signal_time_yield(1);
            yield_current_task();
        }
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            break;
        }
    }

    task.set_interruptible(false);
    Err(Errno::EINTR)
}

/// 向指定进程排队一份携带用户 siginfo 的信号。
///
/// 校验 signum、目标/发送权限和用户 siginfo 的 signo/code 后，实时信号按目标进程
/// RLIMIT_SIGPENDING 原子预留额度，标准信号仍按合并语义。copyin 或额度失败不入队；
/// 成功后由进程级投递路径选择可唤醒线程。
pub fn sys_rt_sigqueueinfo(
    tgid: usize,
    signum: i32,
    uinfo: *const LinuxSigInfo,
) -> SysResult<usize> {
    if tgid == 0 || tgid > isize::MAX as usize {
        return Err(Errno::EINVAL);
    }
    let sig = Sig::from(signum);
    if signum != 0 && !sig.is_valid() {
        return Err(Errno::EINVAL);
    }
    let process = PROCESS_MANAGER.get(tgid).ok_or(Errno::ESRCH)?;
    let current = current_task().expect("[kernel] current task is None.");
    if current.euid() != 0
        && current.euid() != process.uid()
        && current.euid() != process.suid()
        && current.uid() != process.uid()
        && current.uid() != process.suid()
    {
        return Err(Errno::EPERM);
    }
    let mut linux_info = LinuxSigInfo::default();
    copy_from_user(&mut linux_info as *mut LinuxSigInfo, uinfo, 1)?;
    // 与 kill(2) 一样，零号信号只做参数和权限检查，不实际入队。
    // 若把 Sig(0) 传入 pending 集合代码，其从一开始计数的信号索引会发生下溢。
    if signum == 0 {
        return Ok(0);
    }
    if tgid != current.tgid() && linux_info.si_code >= 0 {
        return Err(Errno::EPERM);
    }
    let mut siginfo = SigInfo::from(linux_info);
    siginfo.signo = signum;
    if let Some(task) = process.signal_target() {
        let limit = task
            .rlimit(crate::task::RLIMIT_SIGPENDING)
            .map(|(cur, _)| cur)
            .unwrap_or(usize::MAX);
        if !task.try_receive_siginfo(siginfo, false, limit) {
            return Err(Errno::EAGAIN);
        }
    }
    Ok(0)
}

/// 查询或替换一个信号的处理动作，并保持失败原子性。
///
/// act 先完整 copyin、校验 flags/restorer/不可捕获信号，oldact 也在修改表前完成 copyout；
/// 任一指针错误都不改变 handler。新 mask 自动移除 SIGKILL/SIGSTOP，SA_* 语义在统一
/// `handle_signal`/sigreturn 路径落实。
pub fn sys_sigaction(
    signum: i32,
    act: *const u8,
    oldact: *mut u8,
    sigsetsize: usize,
) -> SysResult<usize> {
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return Err(Errno::EINVAL);
    }
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

    // 写回 oldact 或修改处理器表之前，先准备好全部输入。这样无效的 act/oldact 指针
    // 不会发布半完成操作，并满足系统调用范围的失败原子性约束。
    let prepared_action = if act.is_null() {
        None
    } else {
        let mut new_user_action: UserSigAction = unsafe { core::mem::zeroed() };
        copy_from_user(&mut new_user_action as *mut UserSigAction, act_ptr, 1)?;
        let mut new_action = sigaction_from_user(new_user_action);
        new_action.mask.remove_signal(Sig::SIGKILL);
        new_action.mask.remove_signal(Sig::SIGSTOP);
        Some(new_action)
    };

    // 写回旧动作
    if !oldact.is_null() {
        let old_action = task.op_sig_handler(|handler| handler.get(sig));
        let old_user_action = sigaction_to_user(old_action);
        copy_to_user(oldact_ptr, &old_user_action as *const UserSigAction, 1)?;
    }

    if let Some(new_action) = prepared_action {
        task.op_sig_handler_mut(|handler| handler.update(sig, new_action));
    }

    Ok(0)
}

/// 查询或原子更新当前线程的信号阻塞掩码。
///
/// 新掩码先完整 copyin 并按 BLOCK/UNBLOCK/SETMASK 计算，SIGKILL 与 SIGSTOP 始终清除；
/// oldset 在状态修改前写回，任何 EFAULT 都不得提交新掩码。掩码变化后 pending 信号是否
/// 可投递由统一返回用户态路径重新判断，不在持有 pending 锁时直接运行 handler。
pub fn sys_sigprocmask(
    how: usize,
    set: usize,
    oldset: usize,
    sigsetsize: usize,
) -> SysResult<usize> {
    const SIG_BLOCK: usize = 0;
    const SIG_UNBLOCK: usize = 1;
    const SIG_SETMASK: usize = 2;

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
        if how > SIG_SETMASK {
            return Err(Errno::EINVAL);
        }
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

/// 从用户信号帧恢复寄存器、信号掩码和执行栈，完成 handler 返回。
///
/// 以当前用户 SP 上的 FrameFlags 区分普通帧与 RT 帧，完整 copyin 到内核临时对象后才改写
/// TrapContext。恢复的 mask 必须再次清除不可屏蔽的 SIGKILL/SIGSTOP；同时结束 alt-stack
/// 活动状态并清除内核记录的信号帧地址。无效标记或用户地址返回 EFAULT，不能按任意布局
/// 解释并把攻击者数据直接装入特权返回上下文。
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
    let Some((sig, siginfo, process_scope)) = task.peek_pending_in_set(wanted_set) else {
        return Ok(None);
    };

    // 消费信号前先校验并写入用户态结果；发生 EFAULT 时，Linux 会保留 pending 信号，
    // 供后续等待再次取得。
    if info_ptr != 0 {
        let user_siginfo: LinuxSigInfo = siginfo.into();
        copy_to_user(
            info_ptr as *mut LinuxSigInfo,
            &user_siginfo as *const LinuxSigInfo,
            1,
        )?;
    }
    // 本线程校验用户地址期间，另一线程可能消费进程级 pending 信号。
    // 此时不能误消费更新的同号信号，也不能重复报告旧信号，应重新选择。
    if !task.consume_pending_signal(sig, siginfo, process_scope) {
        return Ok(None);
    }
    task.clear_interrupted();
    Ok(Some(sig.raw() as usize))
}

/// 同步等待并消费集合中的一个 pending 信号，可选相对超时。
///
/// wanted set 通常已被线程信号掩码阻塞；本函数不能临时解屏蔽它，否则普通返回用户态路径
/// 会抢先调用 handler。选择信号后先校验并写回 siginfo，再以 identity 精确消费对应实例，
/// EFAULT 或并发线程抢先消费时不会误删更新的同号实时信号。
///
/// 需要睡眠时发布专用 sigtimedwait mask、interruptible 状态和可选 deadline，随后在 Blocked
/// 可见后复查 wanted pending、普通可投递信号和超时，关闭丢失唤醒窗口。wanted 信号成功返回
/// 其编号，其他可投递信号返回 EINTR，期限到达返回 EAGAIN；所有出口必须清除专用等待状态。
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
    task.set_sigtimedwait_mask(wanted_set);
    if timeout_ptr != 0 {
        // 3a. 有限等待
        let mut timeout = TimeSpec::default();
        let timeout_result = copy_from_user(
            &mut timeout as *mut TimeSpec,
            timeout_ptr as *const TimeSpec,
            1,
        );
        if let Err(err) = timeout_result {
            task.clear_sigtimedwait_mask();
            task.set_interruptible(false);
            return Err(err);
        }
        let total_ms = match timeout.checked_duration_ms() {
            Some(total_ms) => total_ms,
            None => {
                task.clear_sigtimedwait_mask();
                task.set_interruptible(false);
                return Err(Errno::EINVAL);
            }
        };

        // timeout == 0 → 立即轮询返回 EAGAIN
        if timeout.is_zero() {
            task.clear_sigtimedwait_mask();
            task.set_interruptible(false);
            return Err(Errno::EAGAIN);
        }

        let deadline_ms = get_timeout_ms().saturating_add(total_ms);

        loop {
            // 检查信号
            match take_sigtimedwait_signal(wanted_set, info_ptr, &task) {
                Ok(Some(sig)) => {
                    info!("[sys_rt_sigtimedwait] received signal: {}", sig);
                    task.clear_sigtimedwait_mask();
                    task.set_interruptible(false);
                    return Ok(sig);
                }
                Ok(None) => {}
                Err(err) => {
                    task.clear_sigtimedwait_mask();
                    task.set_interruptible(false);
                    return Err(err);
                }
            }

            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.clear_sigtimedwait_mask();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }

            // 检查超时
            if get_timeout_ms() >= deadline_ms {
                info!("[sys_rt_sigtimedwait] timeout");
                task.clear_sigtimedwait_mask();
                task.set_interruptible(false);
                return Err(Errno::EAGAIN);
            }

            if !prepare_current_task_blocked() {
                crate::perf::signal_time_yield(1);
                yield_current_task();
                continue;
            }
            super::register_task_timeout(task.tid(), deadline_ms);

            // 信号或超时可能在上述检查之后、阻塞状态可见之前获胜；
            // 绝不能在这个竞争窗口中错误睡眠。
            let wanted_pending = task.has_pending_in_set(wanted_set);
            if task.is_ready()
                || wanted_pending
                || task.check_signal_interrupt()
                || task.is_interrupted()
                || get_timeout_ms() >= deadline_ms
            {
                remove_task(task.tid());
                task.set_running();
            } else {
                switch_to_next_task();
            }
            super::finish_task_timeout(task.tid());
        }
    } else {
        // 3b. 无限等待
        info!("[sys_rt_sigtimedwait] waiting indefinitely");
        loop {
            match take_sigtimedwait_signal(wanted_set, info_ptr, &task) {
                Ok(Some(sig)) => {
                    info!("[sys_rt_sigtimedwait] received signal: {}", sig);
                    task.clear_sigtimedwait_mask();
                    task.set_interruptible(false);
                    return Ok(sig);
                }
                Ok(None) => {}
                Err(err) => {
                    task.clear_sigtimedwait_mask();
                    task.set_interruptible(false);
                    return Err(err);
                }
            }

            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.clear_sigtimedwait_mask();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }

            if !prepare_current_task_blocked() {
                crate::perf::signal_time_yield(1);
                yield_current_task();
                continue;
            }
            let wanted_pending = task.has_pending_in_set(wanted_set);
            if task.is_ready()
                || wanted_pending
                || task.check_signal_interrupt()
                || task.is_interrupted()
            {
                remove_task(task.tid());
                task.set_running();
            } else {
                switch_to_next_task();
            }
        }
    }
}
