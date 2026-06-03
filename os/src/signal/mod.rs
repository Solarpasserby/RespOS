pub mod sig_handler;
pub mod sig_info;
pub mod sig_stack;
pub mod sig_struct;
use crate::config::TRAMPOLINE;
use crate::mm::copy_to_user;

use crate::task::{current_task, exit_and_run_next};
use sig_handler::{SigAction, SigActionFlag};
pub use sig_info::{LinuxSigInfo, SiField, SigInfo};
use sig_stack::{SigContext, SignalStack, UContext};
pub use sig_struct::{FrameFlags, MAX_SIGNUM, Sig, SigFrame, SigRTFrame, SigSet};

// 每次 trap 返回用户态前调用，处理一个未决信号。
//信号是异步的——进程可能在任何时刻收到信号，但处理时机必须统一。内核选在每个 trap 返回用户态之前检查信号，
//此时 TrapContext已经准备好了，改它就能劫持返回路径。
//如果进程没有陷入过内核（一直在用户态跑），那等下一次 trap（定时器中断、系统调用等）自然会检查。
pub fn handle_signal() {
    let task = current_task().unwrap();

    while let Some((sig, siginfo)) = task.op_sig_pending_mut(|p| p.fetch_signal()) {
        let old_mask = task.op_sig_pending(|p| p.mask);
        let action = task.op_sig_handler(|h| h.get(sig));

        if !action.is_user() {
            // sa_handler == 1 → SIG_IGN：忽略，什么也不做
            // sa_handler == 0 → SIG_DFL：查该信号的默认行为
            if action.sa_handler == 0 {
                use sig_handler::ActionType;
                match ActionType::default(sig) {
                    ActionType::Ignore => {}
                    ActionType::Term => {
                        // TODO: Term 和 Core 当前等价，后续 Core 应做 core dump
                        exit_and_run_next(sig.raw() & 0x7F);
                    }
                    ActionType::Core => {
                        // TODO: Core dump：先将进程内存写成 ELF core 文件，再终止
                        exit_and_run_next(sig.raw() & 0x7F);
                    }
                    ActionType::Stop => {
                        // TODO: 将当前线程置为 Stopped 状态，发 SIGCHLD 给父进程
                    }
                    ActionType::Cont => {
                        // TODO: 恢复当前线程为 Running 状态，发 SIGCHLD 给父进程
                    }
                }
            }
        } else {
            let trap_cx = task.get_trap_cx();

            // SA_NODEFER：handler 执行期间不屏蔽当前信号自身
            if !action.flags.contains(SigActionFlag::SA_NODEFER) {
                task.op_sig_pending_mut(|p| p.add_mask(sig));
            }
            // 加上 action 里指定的额外屏蔽集
            task.op_sig_pending_mut(|p| p.add_mask_sigset(action.mask));

            // 决定 handler 在哪个栈上跑
            let mut user_sp = if action.flags.contains(SigActionFlag::SA_ONSTACK) {
                task.sigstack()
                    .map_or(trap_cx.get_sp(), |s| s.ss_sp + s.ss_size)
            } else {
                trap_cx.get_sp()
            };

            // 栈上留出 SigContext 空间（16 字节对齐）
            let ctx_size = core::mem::size_of::<SigContext>();
            user_sp = (user_sp - ctx_size) & !0xF;

            // 保存当前寄存器快照到用户栈
            let sig_ctx = SigContext {
                x: trap_cx.x,
                sepc: trap_cx.get_sepc(),
                mask: old_mask,
                info: 0, // 0 = 普通 handler；1 = SA_SIGINFO handler
            };
            if copy_to_user(user_sp as *mut SigContext, &sig_ctx as *const SigContext, 1).is_err() {
                error!(
                    "[handle_signal] copy_to_user failed for signal {}, terminating task",
                    sig.raw()
                );
                exit_and_run_next(sig.raw() & 0x7F);
            }

            // 决定向用户栈压入什么：
            if action.flags.contains(SigActionFlag::SA_SIGINFO) {
                // ===== SA_SIGINFO 路径 =====
                // handler 签名: void handler(int sig, siginfo_t *info, void *ucontext)

                // 1. 制作 SigContext（寄存器快照）
                let sig_context = SigContext {
                    x: trap_cx.x,
                    sepc: trap_cx.sepc,
                    mask: old_mask,
                    info: 1, // ← 标记 RT 帧，sigreturn 据此恢复
                };

                // 2. 创建 LinuxSigInfo（用户态可读的 siginfo_t）
                user_sp -= core::mem::size_of::<LinuxSigInfo>();
                user_sp &= !0xF; // 16 字节对齐
                let siginfo_sp = user_sp;
                trap_cx.set_a1(siginfo_sp); // a1 = siginfo_t *info
                let linux_siginfo: LinuxSigInfo = siginfo.into();

                // 3. 创建 UContext
                user_sp -= core::mem::size_of::<UContext>();
                user_sp &= !0xF;
                let ucontext_sp = user_sp;
                trap_cx.set_a2(ucontext_sp); // a2 = ucontext_t *uc
                let ucontext = UContext {
                    uc_flags: 0,
                    uc_link: 0,
                    uc_stack: SignalStack::default(),
                    uc_sigmask: old_mask,
                    uc_sig: [0; 16],
                    uc_mcontext: sig_context,
                };

                // 4. 创建 FrameFlags（栈底标记，sigreturn 用来区分帧类型）
                user_sp -= core::mem::size_of::<FrameFlags>();
                user_sp &= !0xF;

                // 5. 一次性 copy_to_user 整个 SigRTFrame
                let sig_rt_frame = SigRTFrame {
                    flag: FrameFlags::rt_flag(),
                    ucontext,
                    siginfo: linux_siginfo,
                };
                if copy_to_user(
                    user_sp as *mut SigRTFrame,
                    &sig_rt_frame as *const SigRTFrame,
                    1,
                )
                .is_err()
                {
                    error!(
                        "[handle_signal] copy_to_user failed for signal {}, terminating task",
                        sig.raw()
                    );
                    exit_and_run_next(sig.raw() & 0x7F);
                }
            } else {
                // ===== 普通路径
                // 只是多了一个 FrameFlags 标记
                user_sp -= core::mem::size_of::<FrameFlags>();
                user_sp &= !0xF;

                let sig_frame = SigFrame {
                    flag: FrameFlags::normal_flag(),
                    sigcontext: sig_ctx, // info = 0
                };
                if copy_to_user(user_sp as *mut SigFrame, &sig_frame as *const SigFrame, 1).is_err()
                {
                    error!(
                        "[handle_signal] copy_to_user failed for signal {}, terminating task",
                        sig.raw()
                    );
                    exit_and_run_next(sig.raw() & 0x7F);
                }
            }

            // TODO: SA_RESTART 路径
            //   被信号打断的系统调用应返回 ERESTARTSYS，由内核自动重试。

            // 修改 trapframe：sret 后进入用户 handler
            trap_cx.set_a0(sig.raw() as usize);
            trap_cx.set_ra(TRAMPOLINE);
            task.set_sig_context_addr(user_sp);
            trap_cx.set_sp(user_sp);
            trap_cx.set_sepc(action.sa_handler);
            if action.flags.contains(SigActionFlag::SA_RESETHAND) {
                task.op_sig_handler_mut(|h| h.update(sig, SigAction::new(sig)));
            }
        }
        break; // 一次只处理一个信号
    }
}
