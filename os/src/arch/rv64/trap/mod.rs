// os/src/trap.rs

//! RISC-V 用户/内核异常、中断与 syscall 入口。
//!
//! trap 汇编负责在用户栈和内核栈之间切换并保存 `TrapContext`，Rust 分派器处理 ecall、
//! page fault、timer、软件 IPI 和非法指令。返回用户态前再统一处理 signal、抢占和
//! `SA_RESTART`，所以 syscall 实现本身不应直接改写 trap PC。
//!
//! 用户 ecall 进入后 `sepc` 只前进一次；sigreturn 使用 frame 中恢复的上下文。page fault
//! 将 load/store/instruction 原因和用户 SP 传给 MM，以区分 COW、lazy、SIGBUS、SIGSEGV 与
//! 合法的单页 grow-down。内核态 trap 默认处于受控中断状态，但这不意味着任意锁序都安全：
//! timer/IPI 和 scheduler 路径仍须遵守 no-irq lock 与 context handoff 协议。
//!
//! `sret` 前必须保持 kernel trap vector，清除不可信的 SIE，并在通用/浮点状态恢复完毕后
//! 才切换 user vector；否则恢复过程中的中断会把半完成上下文当作完整用户状态。

mod context;

use super::timer::set_next_ti_trigger;
use crate::signal::{SiField, Sig, SigInfo};
use crate::syscall::*;
use crate::task::{current_task, exit_and_run_next, handle_signals, preempt_current_task};
use core::arch::global_asm;
#[cfg(feature = "debug_traces")]
use core::sync::atomic::{AtomicUsize, Ordering};
use riscv::register::{
    mtvec::TrapMode,
    scause::{self, Exception, Interrupt, Trap},
    sie,
    sstatus::{self, SPP},
    stval, stvec,
};

pub use context::TrapContext;

#[cfg(feature = "debug_traces")]
static LAST_LD_TRACE_MS: AtomicUsize = AtomicUsize::new(0);

global_asm!(include_str!("trap.S"));

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PageFaultCause {
    Instruction,
    Load,
    Store,
}

fn page_fault_cause(cause: Trap) -> Option<PageFaultCause> {
    match cause {
        Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::InstructionPageFault) => Some(PageFaultCause::Instruction),
        Trap::Exception(Exception::LoadFault) | Trap::Exception(Exception::LoadPageFault) => {
            Some(PageFaultCause::Load)
        }
        Trap::Exception(Exception::StoreFault) | Trap::Exception(Exception::StorePageFault) => {
            Some(PageFaultCause::Store)
        }
        _ => None,
    }
}

unsafe extern "C" {
    fn __trap_from_user();
    fn __trap_from_kernel();
    pub fn __restore() -> !;
}

pub fn init() {
    let mut sstatus = sstatus::read();
    sstatus.set_spp(SPP::Supervisor);
    unsafe {
        stvec::write(__trap_from_kernel as usize, TrapMode::Direct);
    }
}

pub fn enable_timer_interrupt() {
    unsafe {
        sie::set_stimer();
        sie::set_ssoft();
        // HSM 启动的次 hart 不保证继承 boot hart 的 SIE；仅设置
        // sie.STIE 不会让 supervisor timer 到达 trap vector。
        sstatus::set_sie();
    }
}

/// RV64 用户态陷入的统一分派入口。
///
/// 汇编入口已经把完整用户寄存器保存到当前任务内核栈上的 `TrapContext`。本函数按 scause
/// 分派系统调用、用户缺页、非法指令、断点、定时器和 IPI：系统调用先推进 sepc，并仅为
/// 明确可重启的零进展 EINTR 保留原 arg0；缺页交给 `MemorySet`，I/O/ENOSPC 失败投递 SIGBUS，
/// 其他非法地址投递 SIGSEGV；定时器只在安全点推进全局期限后实施抢占。
///
/// 所有普通分支最终都经过统一信号投递和 CPU 记账收尾。新增异常类型不得直接绕过
/// `handle_signals` 返回用户态，也不得在任意内核持锁点执行全局 timer/scheduler 工作。
#[unsafe(no_mangle)]
pub fn trap_handler(cx: &mut TrapContext) {
    crate::perf::user_trap(1);
    if let Some(task) = current_task() {
        task.enter_kernel_accounting();
    }
    // 设置状态寄存器，使内核可以访问用户数据
    unsafe {
        sstatus::set_sum();
    }
    let scause = scause::read();
    let stval = stval::read();
    let mut restart_syscall_arg0 = None;
    match scause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            let syscall_id = cx.x[17];
            let syscall_args = [cx.x[10], cx.x[11], cx.x[12], cx.x[13], cx.x[14], cx.x[15]];
            let syscall_is_restartable = is_restartable_syscall(syscall_id, &syscall_args);
            crate::perf::user_syscall_trap(1);
            cx.sepc += 4; // 异常处理完成后直接执行后续指令
            let ret = syscall(syscall_id, syscall_args);
            if ret == Err(Errno::EINTR) && syscall_is_restartable {
                restart_syscall_arg0 = Some(syscall_args[0]);
            }
            cx.x[10] = match ret {
                Ok(ret) => ret,
                Err(err) => err.as_ret() as usize,
            };
        }
        Trap::Exception(Exception::StoreFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::InstructionPageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::LoadPageFault) => {
            crate::perf::user_page_fault_trap(1);
            let page_fault_cause = page_fault_cause(scause.cause()).unwrap();
            let result = current_task()
                .expect("[kernel] current task is None.")
                .op_memory_set_write(|memory_set| {
                    memory_set.handle_page_fault(page_fault_cause, stval, Some(cx.get_sp()))
                });
            if let Ok(outcome) = result {
                if let Some(task) = current_task() {
                    match outcome {
                        crate::mm::PageFaultOutcome::Minor => task.note_minor_fault(),
                        crate::mm::PageFaultOutcome::Major => task.note_major_fault(),
                        crate::mm::PageFaultOutcome::Retry => {}
                    }
                    if outcome != crate::mm::PageFaultOutcome::Retry {
                        let resident =
                            task.op_memory_set_read(|memory_set| memory_set.resident_page_count());
                        task.note_maxrss_pages(resident);
                    }
                }
            }
            if let Err(err) = result {
                #[cfg(feature = "fault_trace")]
                if let Some(task) = current_task() {
                    println!(
                        "[user-fault] hart={} tid={} tgid={} cause={:?} sepc={:#x} stval={:#x} sp={:#x} ra={:#x} err={:?}",
                        crate::arch::smp::current_hart_id(),
                        task.tid(),
                        task.tgid(),
                        page_fault_cause,
                        cx.sepc,
                        stval,
                        cx.x[2],
                        cx.x[1],
                        err
                    );
                }
                let sig = if matches!(err, Errno::EIO | Errno::ENOSPC) {
                    Sig::SIGBUS
                } else {
                    Sig::SIGSEGV
                };
                let siginfo = SigInfo::new(sig.raw(), SigInfo::KERNEL, SiField::None);
                current_task()
                    .expect("[kernel] current task is None.")
                    .receive_siginfo(siginfo, true);
            }
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            let instruction = unsafe { (cx.sepc as *const u32).read_unaligned() };
            let task = current_task().expect("[kernel] current task is None.");
            println!(
                "[illegaltrace] hart={} tid={} tgid={} sepc={:#x} instruction={:#010x} stval={:#x}",
                crate::arch::smp::current_hart_id(),
                task.tid(),
                task.tgid(),
                cx.sepc,
                instruction,
                stval
            );
            // 非法指令属于同步用户异常。向触发异常的线程投递 SIGILL，
            // 使已安装的处理器有机会检查并恢复；若没有处理器，普通信号路径会执行
            // 进程范围的默认 Core 动作。只杀死当前线程会使 pthread join 等待者悬空，
            // 也不符合 Linux 的致命信号语义。
            let siginfo = SigInfo::new(Sig::SIGILL.raw(), SigInfo::KERNEL, SiField::None);
            task.receive_siginfo(siginfo, true);
        }
        Trap::Exception(Exception::Breakpoint) => {
            println!(
                "[kernel] Breakpoint in application at sepc={:#x}, kernel killed it.",
                cx.sepc
            );
            exit_and_run_next(-4);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            crate::perf::user_timer_trap(1);
            crate::perf::sample_concurrency();
            set_next_ti_trigger();
            #[cfg(feature = "debug_traces")]
            {
                if let Some(task) = current_task() {
                    if task.tgid() == 20 {
                        let now = crate::timer::get_time_ms();
                        let last = LAST_LD_TRACE_MS.load(Ordering::Relaxed);
                        if now.saturating_sub(last) >= 1_000
                            && LAST_LD_TRACE_MS
                                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                                .is_ok()
                        {
                            println!(
                                "[ldtrace] hart={} tid={} sepc={:#x}",
                                crate::arch::smp::current_hart_id(),
                                task.tid(),
                                cx.sepc
                            );
                        }
                    }
                }
            }
            if crate::arch::smp::is_timer_service_hart() {
                crate::timer::await_task_timer_deadline();
                check_all_task_timers();
            }
            preempt_current_task();
        }
        Trap::Interrupt(Interrupt::SupervisorSoft) => {
            crate::perf::user_ipi_trap(1);
            crate::arch::smp::acknowledge_ipi();
            crate::timer::rearm_task_timer_request();
        }
        _ => {
            panic!(
                "Unsupported trap {:?}, stval = {:#?}!",
                scause.cause(),
                stval
            );
        }
    };
    handle_signals(restart_syscall_arg0);
    if let Some(task) = current_task() {
        task.leave_kernel_accounting();
    }
    return;
}

#[unsafe(no_mangle)]
/// RV64 内核态 trap 分派器，只处理可以在无任务信号语义下安全完成的异常与中断。
///
/// 内核缺页、非法指令和意外 ecall 属于内核不变量破坏并立即 panic；定时器/IPI 只应答硬件、
/// 重设期限，并仅在 idle 安全点执行会触碰全局任务/信号表的超时工作。该入口不能调用
/// 用户态 `handle_signal`，也不能在任意被中断的持锁临界区实施任务抢占。
pub fn kernel_trap_handler(cx: &mut TrapContext) {
    let scause = scause::read();
    match scause.cause() {
        Trap::Exception(Exception::Breakpoint) => {
            info!("Breakpoint at 0x{:x}", cx.sepc);
            cx.sepc += 2;
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            panic!("IllegalInstruction at 0x{:x}", cx.sepc);
        }
        Trap::Exception(Exception::LoadPageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::StoreFault) => {
            panic!(
                "page fault in kernel, sepc = {:#x}, bad addr = {:#x}, scause = {:?}",
                cx.sepc,
                stval::read(),
                scause.cause()
            );
        }
        Trap::Exception(Exception::InstructionPageFault) => {
            panic!(
                "Instruction page fault at 0x{:x}, badaddr = {:#x}",
                cx.sepc,
                stval::read()
            );
        }
        Trap::Exception(Exception::UserEnvCall) => {
            panic!("UserEnvCall from kernel!");
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            crate::perf::sample_concurrency();
            // SBI 定时器中断在编程一个更晚的期限前会一直保持 pending。
            // 长系统调用路径可以合法跨越 tick；通过设置下一 tick 并处理到期项来应答中断。
            // 内核代码中的调度仍只发生在显式安全点。
            set_next_ti_trigger();
            // 全局超时工作会访问任务、信号和定时器注册表，不能重入任意内核临界区。
            // 内核态 tick 仅在本硬件线程处于每 CPU idle 上下文
            //（`current_task == None`，不持任务锁）时处理；直接从用户态进入的 tick
            // 由上方 trap_handler 的安全路径处理。
            //
            // SMP 早期阶段仍由启动核独占全局定时服务；次级核只重设本地 tick。
            if crate::arch::smp::is_timer_service_hart() && crate::task::current_task().is_none() {
                crate::timer::await_task_timer_deadline();
                check_all_task_timers();
            }
        }
        Trap::Interrupt(Interrupt::SupervisorSoft) => {
            crate::arch::smp::acknowledge_ipi();
            crate::timer::rearm_task_timer_request();
        }
        _ => {
            panic!(
                "Unsupported trap {:?}, stval = {:#x}!",
                scause.cause(),
                stval::read()
            );
        }
    }
}
