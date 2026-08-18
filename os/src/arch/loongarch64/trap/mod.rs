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

/// 真机 trap 诊断：无锁直写 uncached UART，避免 panic 路径的 console 锁死锁与
/// 预编译 core fmt（+ual 非对齐代码）在 trap 上下文里的二次异常。只打印一次。
#[cfg(feature = "board_ls2k1000")]
fn board_trap_diag(tag: &str, cx: &TrapContext) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static TRAP_DIAG_PRINTED: AtomicBool = AtomicBool::new(false);
    if TRAP_DIAG_PRINTED.swap(true, Ordering::Relaxed) {
        return;
    }
    crate::arch::sbi::early_print("[trap] ");
    crate::arch::sbi::early_print(tag);
    crate::arch::sbi::early_print(" est=");
    crate::arch::sbi::early_print_hex(estat::read());
    crate::arch::sbi::early_print(" era=");
    crate::arch::sbi::early_print_hex(cx.era);
    crate::arch::sbi::early_print(" badv=");
    crate::arch::sbi::early_print_hex(badv::read());
    crate::arch::sbi::early_print(" badi=");
    crate::arch::sbi::early_print_hex(read_badi());
    crate::arch::sbi::early_print("\n");
}

#[cfg(not(feature = "board_ls2k1000"))]
fn board_trap_diag(_tag: &str, _cx: &TrapContext) {}

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
                board_trap_diag("user-adem-unemulated", cx);
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
            board_trap_diag("user-unsupported-interrupt", cx);
            panic!(
                "[kernel] Unsupported interrupt: {:?}, era = {:#x}",
                interrupt, cx.era
            );
        }
        cause => {
            let badv = badv::read();
            board_trap_diag("user-unsupported-trap", cx);
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

/// 模拟访存前先让目标页可见（懒分配栈/COW 等）。否则逐字节读写会在内核态触发
/// PIS 直接 panic（现场赛 exec 后 musl 对懒分配栈页的非对齐 store 就死在这里）。
/// 只处理用户地址；内核地址（直接映射/内核堆）恒已映射，跳过。
fn fault_in_emulated_access(
    cx: &TrapContext,
    addr: usize,
    len: usize,
    cause: PageFaultCause,
) -> bool {
    use crate::config::{KERNEL_BASE, PAGE_SIZE};
    if addr >= KERNEL_BASE {
        return true;
    }
    let Some(task) = current_task() else {
        return true;
    };
    let end = addr.saturating_add(len.saturating_sub(1));
    let mut va = addr & !(PAGE_SIZE - 1);
    task.op_memory_set_write(|memory_set| {
        while va <= end {
            if memory_set
                .handle_page_fault(cause, va, Some(cx.get_sp()))
                .is_err()
            {
                return false;
            }
            va += PAGE_SIZE;
        }
        true
    })
}

/// 内核/用户态非对齐访存（ADEM）模拟。LA264 UAL=0，lwext4 C 库、预编译 core/alloc
/// 等按 +ual 编译的代码会在真机触发 ADEM；这里逐字节模拟 ld/st，成功后把 ERA 推进
/// 到下一条指令。覆盖 2RI12（ld/st）、3R（ldx/stx）、2RI14（ll/sc/ldptr/stptr）三组
/// 整数访存指令。返回 false 表示不是可模拟的指令（交给上层）。
fn emulate_unaligned_access(cx: &mut TrapContext) -> bool {
    let inst = unsafe { core::ptr::read_volatile(cx.era as *const u32) } as usize;

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

    // 2RI12：opcode=inst[31:22]，si12=inst[21:10]，rj=inst[9:5]，rd=inst[4:0]。
    let opcode10 = (inst >> 22) & 0x3ff;
    if (0xa0..=0xaa).contains(&opcode10) {
        let si12 = ((inst >> 10) & 0xfff) as i16 as isize;
        let rj = (inst >> 5) & 0x1f;
        let rd = inst & 0x1f;
        let addr = (cx.x[rj] as isize).wrapping_add(si12) as usize;

        let (size, is_store) = match opcode10 {
            0xa0 | 0xa8 => (1, false), // ld.b / ld.bu
            0xa1 | 0xa9 => (2, false), // ld.h / ld.hu
            0xa2 | 0xaa => (4, false), // ld.w / ld.wu
            0xa3 => (8, false),        // ld.d
            0xa4 => (1, true),         // st.b
            0xa5 => (2, true),         // st.h
            0xa6 => (4, true),         // st.w
            0xa7 => (8, true),         // st.d
            _ => return false,
        };
        let cause = if is_store {
            PageFaultCause::Store
        } else {
            PageFaultCause::Load
        };
        if !fault_in_emulated_access(cx, addr, size, cause) {
            return false;
        }
        if is_store {
            write_bytes(addr, cx.x[rd], size);
        } else {
            let val = match opcode10 {
                0xa0 => (read_bytes(addr, 1) as u8 as i8) as isize as usize,
                0xa1 => (read_bytes(addr, 2) as u16 as i16) as isize as usize,
                0xa2 => (read_bytes(addr, 4) as u32 as i32) as isize as usize,
                0xa3 => read_bytes(addr, 8),
                0xa8 => read_bytes(addr, 1),
                0xa9 => read_bytes(addr, 2),
                0xaa => read_bytes(addr, 4),
                _ => unreachable!(),
            };
            if rd != 0 {
                cx.x[rd] = val;
            }
        }
        cx.era += 4;
        return true;
    }

    // 3R：ldx/stx（opcode=inst[31:15]，rk=inst[14:10]，rj=inst[9:5]，rd=inst[4:0]），addr=rj+rk。
    let opcode17 = (inst >> 15) & 0x1_ffff;
    if (0x7000..=0x7038).contains(&opcode17) {
        let rk = (inst >> 10) & 0x1f;
        let rj = (inst >> 5) & 0x1f;
        let rd = inst & 0x1f;
        let addr = cx.x[rj].wrapping_add(cx.x[rk]);
        let (size, is_store) = match opcode17 {
            0x7000 | 0x7004 => (1, false), // ldx.b / ldx.bu
            0x7008 | 0x700c => (2, false), // ldx.h / ldx.hu
            0x7010 | 0x7014 => (4, false), // ldx.w / ldx.wu
            0x7018 => (8, false),          // ldx.d
            0x7020 => (1, true),           // stx.b
            0x7028 => (2, true),           // stx.h
            0x7030 => (4, true),           // stx.w
            0x7038 => (8, true),           // stx.d
            _ => return false,
        };
        let cause = if is_store {
            PageFaultCause::Store
        } else {
            PageFaultCause::Load
        };
        if !fault_in_emulated_access(cx, addr, size, cause) {
            return false;
        }
        if is_store {
            write_bytes(addr, cx.x[rd], size);
        } else {
            let val = match opcode17 {
                0x7000 => (read_bytes(addr, 1) as u8 as i8) as isize as usize,
                0x7004 => read_bytes(addr, 1),
                0x7008 => (read_bytes(addr, 2) as u16 as i16) as isize as usize,
                0x700c => read_bytes(addr, 2),
                0x7010 => (read_bytes(addr, 4) as u32 as i32) as isize as usize,
                0x7014 => read_bytes(addr, 4),
                0x7018 => read_bytes(addr, 8),
                _ => unreachable!(),
            };
            if rd != 0 {
                cx.x[rd] = val;
            }
        }
        cx.era += 4;
        return true;
    }

    // 2RI14：ll/sc/ldptr/stptr（opcode=inst[31:26]，si14=inst[25:10]）。
    let opcode6 = (inst >> 26) & 0x3f;
    if (0x20..=0x27).contains(&opcode6) {
        let v14 = ((inst >> 10) & 0x3fff) as isize;
        let si14 = if v14 & 0x2000 != 0 { v14 - 0x4000 } else { v14 };
        let rj = (inst >> 5) & 0x1f;
        let rd = inst & 0x1f;
        let addr = (cx.x[rj] as isize).wrapping_add(si14) as usize;
        let (size, is_store) = match opcode6 {
            0x20 | 0x24 => (4, false), // ll.w / ldptr.w
            0x22 | 0x26 => (8, false), // ll.d / ldptr.d
            0x21 | 0x25 => (4, true),  // sc.w / stptr.w
            0x23 | 0x27 => (8, true),  // sc.d / stptr.d
            _ => return false,
        };
        let cause = if is_store {
            PageFaultCause::Store
        } else {
            PageFaultCause::Load
        };
        if !fault_in_emulated_access(cx, addr, size, cause) {
            return false;
        }
        if is_store {
            write_bytes(addr, cx.x[rd], size);
            if (opcode6 == 0x21 || opcode6 == 0x23) && rd != 0 {
                cx.x[rd] = 1; // sc 按成功返回 1
            }
        } else {
            let val = if size == 4 {
                (read_bytes(addr, 4) as u32 as i32) as isize as usize
            } else {
                read_bytes(addr, 8)
            };
            if rd != 0 {
                cx.x[rd] = val;
            }
        }
        cx.era += 4;
        return true;
    }

    false
}

#[unsafe(no_mangle)]
pub fn trap_from_kernel(cx: &mut TrapContext) {
    match estat::cause(estat::read()) {
        estat::Trap::Exception(estat::Exception::Breakpoint) => {
            println!("[kernel] Breakpoint at 0x{:x}", cx.era);
            cx.era += 4; // LoongArch break 指令为 4 字节
        }
        estat::Trap::Exception(estat::Exception::IllegalInstruction) => {
            board_trap_diag("kernel-illegal-instr", cx);
            panic!("[kernel] IllegalInstruction at 0x{:x}", cx.era);
        }
        estat::Trap::Exception(estat::Exception::AddressNotAligned) => {
            // LA264 UAL=0，lwext4 C 库等代码的非对齐访存走这里逐字节模拟。
            if !emulate_unaligned_access(cx) {
                board_trap_diag("kernel-adem-unemulated", cx);
                panic!(
                    "[kernel] AddressNotAligned not emulated: era = {:#x}, badv = {:#x}, inst = {:#x}",
                    cx.era,
                    badv::read(),
                    unsafe { core::ptr::read_volatile(cx.era as *const u32) }
                );
            }
        }
        estat::Trap::Exception(exception) if is_page_fault(exception) => {
            board_trap_diag("kernel-page-fault", cx);
            panic!(
                "[kernel] page fault in kernel, era = {:#x}, badaddr = {:#x}, cause = {:?}",
                cx.era,
                badv::read(),
                estat::Trap::Exception(exception)
            );
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            board_trap_diag("kernel-syscall", cx);
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
            board_trap_diag("kernel-unsupported-trap", cx);
            panic!(
                "[kernel] Unsupported trap in kernel: cause = {:?}, era = {:#x}, badv = {:#x}!",
                cause,
                cx.era,
                badv::read()
            );
        }
    }
}
