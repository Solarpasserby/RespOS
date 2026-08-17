// os/src/arch/loongarch64/trap/mod.rs

mod context;

use super::register::{badv, ecfg, eentry, estat};
use super::{
    sbi::clear_timer_interrupt,
    timer::{note_timer_tick, set_next_ti_trigger},
};
use crate::signal::{SiField, Sig, SigInfo};
use crate::syscall::*;
use crate::task::{
    current_task, exit_and_run_next, exit_by_signal_and_run_next, handle_signals,
    preempt_current_task,
};
use core::arch::global_asm;

pub use context::TrapContext;

/// 页错误原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultCause {
    Instruction,
    Load,
    Store,
}

fn page_fault_cause(exception: estat::Exception) -> PageFaultCause {
    match exception {
        estat::Exception::PageInvalidFetch | estat::Exception::PageNonExecutable => {
            PageFaultCause::Instruction
        }
        estat::Exception::PageInvalidStore | estat::Exception::PageModifyFault => {
            PageFaultCause::Store
        }
        _ => PageFaultCause::Load,
    }
}

fn is_page_fault(exception: estat::Exception) -> bool {
    matches!(
        exception,
        estat::Exception::PageInvalidFetch
            | estat::Exception::PageInvalidLoad
            | estat::Exception::PageInvalidStore
            | estat::Exception::PageModifyFault
            | estat::Exception::PageNonReadable
            | estat::Exception::PageNonExecutable
            | estat::Exception::PagePrivilegeIllegal
    )
}

fn handle_user_page_fault(cx: &TrapContext, exception: estat::Exception) {
    let badv = badv::read();
    let cause = page_fault_cause(exception);
    let result = current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_write(|memory_set| {
            memory_set.handle_page_fault(cause, badv, Some(cx.get_sp()))
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
        let task = current_task().expect("[kernel] current task is None.");
        #[cfg(feature = "fault_trace")]
        println!(
            "[user-fault] tid={} tgid={} cause={:?} era={:#x} badi={:#010x} badv={:#x} a0={:#x} a1={:#x} a2={:#x} a3={:#x} sp={:#x} ra={:#x} err={:?}",
            task.tid(),
            task.tgid(),
            cause,
            _cx.era,
            read_badi(),
            badv,
            _cx.x[4],
            _cx.x[5],
            _cx.x[6],
            _cx.x[7],
            _cx.x[3],
            _cx.x[1],
            err
        );
        let sig = if matches!(err, Errno::EIO | Errno::ENOSPC) {
            Sig::SIGBUS
        } else {
            Sig::SIGSEGV
        };
        if task.op_sig_pending(|pending| pending.mask.contain_signal(sig)) {
            exit_by_signal_and_run_next(sig.raw());
        }
        let siginfo = SigInfo::new(sig.raw(), SigInfo::KERNEL, SiField::None);
        task.receive_siginfo(siginfo, true);
    }
}

fn handle_user_syscall(cx: &mut TrapContext) -> Option<usize> {
    let syscall_id = cx.syscall_id();
    let syscall_args = cx.syscall_args();
    let syscall_is_restartable = is_restartable_syscall(syscall_id, &syscall_args);
    cx.era += 4;
    let ret = syscall(syscall_id, syscall_args);
    if syscall_id == SYSCALL_SIGRETURN && ret.is_ok() {
        return None;
    }

    let restart_syscall_arg0 =
        (ret == Err(Errno::EINTR) && syscall_is_restartable).then_some(syscall_args[0]);

    cx.x[4] = match ret {
        Ok(ret) => ret,
        Err(err) => err.as_ret() as usize,
    };
    restart_syscall_arg0
}

global_asm!(include_str!("trap.S"));

unsafe extern "C" {
    fn __trap_from_user();
    fn __trap_from_kernel();
    pub fn __restore() -> !;
}

pub fn init() {
    // 初始化阶段尚未准备用户上下文，异常先进入内核 trap 路径。
    unsafe {
        eentry::write(__trap_from_kernel as usize);
    }
}

pub fn enable_timer_interrupt() {
    unsafe {
        ecfg::enable_timer_interrupt();
        // 内核态保持关中断；用户态通过 PRMD.PIE 在 ERTN 后重新开中断。
        super::register::crmd::set_interrupt_enabled(false);
    }
}

#[inline]
fn read_badi() -> usize {
    let bits: usize;
    unsafe {
        core::arch::asm!("csrrd {}, 0x8", out(reg) bits, options(nomem, nostack));
    }
    bits
}

/// 异常处理入口
#[unsafe(no_mangle)]
pub fn trap_handler(cx: &mut TrapContext) {
    crate::perf::user_trap(1);
    if let Some(task) = current_task() {
        task.enter_kernel_accounting();
    }
    // 未使用过 FP/LSX 的任务在汇编入口跳过扩展状态保存。任务首次
    // 触发 FPD/SXD 后仍采用 eager save/restore，以保持跨核迁移语义简单可靠。
    if cx.extension_state_active() {
        crate::perf::extension_state_eager_save(1);
    }
    let mut restart_syscall_arg0 = None;
    match estat::cause(estat::read()) {
        estat::Trap::Interrupt(estat::Interrupt::Timer) => {
            crate::perf::user_timer_trap(1);
            crate::perf::sample_concurrency();
            clear_timer_interrupt();
            note_timer_tick();
            set_next_ti_trigger();
            if crate::arch::smp::is_timer_service_hart() {
                crate::timer::await_task_timer_deadline();
                check_all_task_timers();
            }
            preempt_current_task();
        }
        estat::Trap::Interrupt(estat::Interrupt::Ipi) => {
            crate::perf::user_ipi_trap(1);
            crate::arch::smp::acknowledge_ipi();
            crate::timer::rearm_task_timer_request();
        }
        // LoongArch ECODE=0 denotes an interrupt. QEMU may withdraw the last
        // ESTAT.IS bit between taking the trap and this CSR read, leaving a
        // zero ECODE with no source bit. Treat that state as a spurious timer
        // edge instead of misclassifying it as an unsupported exception and
        // panicking the whole kernel under sustained user-space activity.
        estat::Trap::Exception(estat::Exception::Unknown(0)) => {
            clear_timer_interrupt();
            set_next_ti_trigger();
            if crate::arch::smp::is_timer_service_hart() {
                crate::timer::await_task_timer_deadline();
                check_all_task_timers();
            }
            preempt_current_task();
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            crate::perf::user_syscall_trap(1);
            restart_syscall_arg0 = handle_user_syscall(cx);
        }
        estat::Trap::Exception(exception) if is_page_fault(exception) => {
            crate::perf::user_page_fault_trap(1);
            handle_user_page_fault(cx, exception);
        }
        estat::Trap::Exception(
            estat::Exception::FloatingPointUnavailable | estat::Exception::SimdUnavailable,
        ) => {
            // __restore enables FP/LSX and retries the faulting instruction.
            cx.activate_extension_state();
        }
        estat::Trap::Exception(estat::Exception::IllegalInstruction) => {
            let inst = read_badi();
            let tid = current_task().map(|task| task.tid()).unwrap_or(usize::MAX);
            println!(
                "[kernel] IllegalInstruction in application, tid = {}, era = {:#x}, badi = {:#x}, kernel killed it.",
                tid, cx.era, inst
            );
            exit_and_run_next(-3);
        }
        estat::Trap::Exception(estat::Exception::Breakpoint) => {
            println!(
                "[kernel] Breakpoint in application at era={:#x}, kernel killed it.",
                cx.era
            );
            exit_and_run_next(-4);
        }
        // LA264 UAL=0，用户态非对齐访存（预编译 core/alloc rlib 里的非对齐 ld/st）会
        // 触发 ADEM。先逐字节模拟；无法模拟的指令编码才按 SIGBUS 结束任务。
        estat::Trap::Exception(
            estat::Exception::AddressError | estat::Exception::AddressNotAligned,
        ) => {
            if !emulate_unaligned_access(cx) {
                let badv = badv::read();
                let tid = current_task().map(|task| task.tid()).unwrap_or(usize::MAX);
                println!(
                    "[kernel] AddressNotAligned in application, tid = {}, era = {:#x}, badv = {:#x}, kernel killed it.",
                    tid, cx.era, badv
                );
                if let Some(task) = current_task() {
                    let siginfo =
                        SigInfo::new(Sig::SIGBUS.raw(), SigInfo::KERNEL, SiField::None);
                    task.receive_siginfo(siginfo, true);
                }
                exit_by_signal_and_run_next(Sig::SIGBUS.raw());
            }
        }
        estat::Trap::Interrupt(interrupt) => {
            panic!(
                "[kernel] Unsupported interrupt: {:?}, era = {:#x}",
                interrupt, cx.era
            );
        }
        cause => {
            let badv = badv::read();
            panic!(
                "Unsupported trap: cause = {:?}, era = {:#x}, badv = {:#x}!",
                cause, cx.era, badv
            );
        }
    }
    handle_signals(restart_syscall_arg0);
    if let Some(task) = current_task() {
        task.leave_kernel_accounting();
    }
}

/// 内核态非对齐访存（ADEM）模拟。LA264 UAL=0，lwext4 C 库等按 +ual 编译的代码会在
/// 真机触发 ADEM；这里逐字节模拟 ld/st，成功后把 ERA 推进到下一条指令。
/// 返回 false 表示不是可模拟的指令（交给上层 panic）。
fn emulate_unaligned_access(cx: &mut TrapContext) -> bool {
    // LoongArch 2RI12 访存指令：opcode=inst[31:22], si12=inst[21:10], rj=inst[9:5], rd=inst[4:0]。
    let inst = unsafe { core::ptr::read_volatile(cx.era as *const u32) } as usize;
    let opcode = (inst >> 22) & 0x3ff;
    let si12 = ((inst >> 10) & 0xfff) as i16 as isize;
    let rj = (inst >> 5) & 0x1f;
    let rd = inst & 0x1f;

    let addr = (cx.x[rj] as isize).wrapping_add(si12) as usize;

    let read_bytes = |addr: usize, n: usize| -> usize {
        let mut v = 0usize;
        for i in 0..n {
            v |= (unsafe { core::ptr::read_volatile((addr + i) as *const u8) } as usize) << (i * 8);
        }
        v
    };
    let write_bytes = |addr: usize, val: usize, n: usize| {
        for i in 0..n {
            unsafe {
                core::ptr::write_volatile((addr + i) as *mut u8, ((val >> (i * 8)) & 0xff) as u8)
            };
        }
    };

    let loaded = match opcode {
        0xa0 => Some((read_bytes(addr, 1) as u8 as i8) as isize as usize), // ld.b
        0xa1 => Some((read_bytes(addr, 2) as u16 as i16) as isize as usize), // ld.h
        0xa2 => Some((read_bytes(addr, 4) as u32 as i32) as isize as usize), // ld.w
        0xa3 => Some(read_bytes(addr, 8)),                                   // ld.d
        0xa8 => Some(read_bytes(addr, 1)),                                   // ld.bu
        0xa9 => Some(read_bytes(addr, 2)),                                   // ld.hu
        0xaa => Some(read_bytes(addr, 4)),                                   // ld.wu
        _ => None,
    };
    if let Some(val) = loaded {
        if rd != 0 {
            cx.x[rd] = val;
        }
        cx.era += 4;
        return true;
    }

    match opcode {
        0xa4 => write_bytes(addr, cx.x[rd], 1), // st.b
        0xa5 => write_bytes(addr, cx.x[rd], 2), // st.h
        0xa6 => write_bytes(addr, cx.x[rd], 4), // st.w
        0xa7 => write_bytes(addr, cx.x[rd], 8), // st.d
        _ => return false,
    }
    cx.era += 4;
    true
}

#[unsafe(no_mangle)]
pub fn trap_from_kernel(cx: &mut TrapContext) {
    match estat::cause(estat::read()) {
        estat::Trap::Exception(estat::Exception::Breakpoint) => {
            println!("[kernel] Breakpoint at 0x{:x}", cx.era);
            cx.era += 4; // LoongArch break 指令为 4 字节
        }
        estat::Trap::Exception(estat::Exception::IllegalInstruction) => {
            panic!("[kernel] IllegalInstruction at 0x{:x}", cx.era);
        }
        estat::Trap::Exception(estat::Exception::AddressNotAligned) => {
            // LA264 UAL=0，lwext4 C 库等代码的非对齐访存走这里逐字节模拟。
            if !emulate_unaligned_access(cx) {
                panic!(
                    "[kernel] AddressNotAligned not emulated: era = {:#x}, badv = {:#x}, inst = {:#x}",
                    cx.era,
                    badv::read(),
                    unsafe { core::ptr::read_volatile(cx.era as *const u32) }
                );
            }
        }
        estat::Trap::Exception(exception) if is_page_fault(exception) => {
            panic!(
                "[kernel] page fault in kernel, era = {:#x}, badaddr = {:#x}, cause = {:?}",
                cx.era,
                badv::read(),
                estat::Trap::Exception(exception)
            );
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            panic!("[kernel] Syscall from kernel!");
        }
        estat::Trap::Interrupt(estat::Interrupt::Timer) => {
            crate::perf::sample_concurrency();
            clear_timer_interrupt();
            note_timer_tick();
            set_next_ti_trigger();
            if crate::arch::smp::is_timer_service_hart() && crate::task::current_task().is_none() {
                crate::timer::await_task_timer_deadline();
                check_all_task_timers();
            }
        }
        estat::Trap::Interrupt(estat::Interrupt::Ipi) => {
            crate::arch::smp::acknowledge_ipi();
            crate::timer::rearm_task_timer_request();
        }
        cause => {
            panic!(
                "[kernel] Unsupported trap in kernel: cause = {:?}, era = {:#x}, badv = {:#x}!",
                cause,
                cx.era,
                badv::read()
            );
        }
    }
}
