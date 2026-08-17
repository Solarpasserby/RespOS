// os/src/trap.rs

//! ### ~~中断~~异常模块
//!
//! 注：应当注意到目前内核台下触发中断会被屏蔽
//! 因此无需担心某些过程是否需要关闭中断

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

/// ~~中断~~异常处理函数
///
/// 用户程序上下文保存于内核栈上，包含用户程序使用的寄存器数据以及系统调用传递的寄存器参数
///
/// 该函数根据 `CSR` 区分不同异常类型，对不同类型异常做不同处理
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
            // IllegalInstruction is a synchronous user exception.  Deliver
            // SIGILL to the faulting thread so an installed handler can
            // inspect/recover from it; the normal signal path applies the
            // process-wide default Core action when no handler is installed.
            // Killing only this thread strands pthread joiners and differs
            // from Linux fatal-signal semantics.
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
            // SBI timer interrupts remain pending until a later deadline is
            // programmed. Long syscall paths can legitimately cross a tick;
            // acknowledge it by arming the next tick and process expirations.
            // Scheduling remains at explicit safe points in kernel code.
            set_next_ti_trigger();
            // Global timeout work touches task/signal/timer registries and
            // must not re-enter an arbitrary kernel critical section.  A
            // kernel-mode tick processes it only when this hart is in the
            // per-CPU idle context (`current_task == None`), where no task
            // lock is held. Ticks taken directly from user mode are handled
            // by trap_handler's safe path above.
            //
            // The boot hart remains the sole global timer service CPU during
            // early SMP; secondary harts only re-arm their local tick.
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
