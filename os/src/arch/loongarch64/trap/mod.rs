// os/src/arch/loongarch64/trap/mod.rs

//! LoongArch 用户/内核异常、中断与 syscall 入口。
//!
//! 汇编入口保存寄存器并构造 `TrapContext`，本模块根据 ESTAT 分派 syscall、页错误、timer、
//! IPI、FP/LSX unavailable 和非法指令。返回用户态前还要完成 signal frame 投递、可能的
//! syscall restart、扩展寄存器恢复以及调度抢占。
//!
//! ERA 对普通 syscall 只前进一次；sigreturn 成功后使用用户 frame 恢复的 PC，不能再次覆盖。
//! 页错误必须把访问类型传给 `MemorySet`，ENOSPC/EIO 型 file fault 映射为 SIGBUS，其余非法
//! 地址映射为 SIGSEGV。FP/LSX 首次使用异常不推进 ERA，而是启用当前任务状态后重试指令。
//!
//! trap 路径处于中断、MM 和 scheduler 的交界，锁内不得执行可能切换任务的操作。secondary
//! hart 在 boot timer service 发布前也不能借 timer trap 提前进入公共 scheduler。

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

/// 把 LoongArch 页异常原因转换为公共缺页类型，并完成 fault 记账与信号选择。
///
/// `MemorySet::handle_page_fault` 在写锁内处理 lazy/COW/page-mkwrite；成功时更新 minor/major
/// 与 RSS 高水位，Retry 表示架构可直接重试。EIO/ENOSPC 表示映射对象后端故障并投递 SIGBUS，
/// 权限或地址错误投递 SIGSEGV。这里只入队信号，实际 handler/default action 由统一返回路径执行。
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
            cx.era,
            read_badi(),
            badv,
            cx.x[4],
            cx.x[5],
            cx.x[6],
            cx.x[7],
            cx.x[3],
            cx.x[1],
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

/// LoongArch64 用户态陷入的统一分派入口。
///
/// 除系统调用、缺页、定时器和 IPI 外，还负责 FP/LSX 首次使用、非法指令以及 ADEM
/// 非对齐访存模拟。扩展状态只在任务已激活时由汇编 eager 保存；Unavailable 异常仅开启状态并
/// 重试原指令。用户缺页和 syscall 与 RV64 共享领域实现，架构层只负责异常原因转换与信号选择。
///
/// QEMU 可能产生来源位已撤销的 ECODE=0 伪中断，该分支必须安全应答而不能 panic。
/// 所有可返回用户态的路径最终统一处理 pending 信号并结束内核态 CPU 记账。
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
        // LoongArch 的 ECODE=0 表示中断。QEMU 可能在陷入发生后、读取 CSR 前撤销最后一个
        // ESTAT.IS 位，于是留下 ECODE 为零却没有来源位的状态。应将它视为伪定时器边沿，
        // 不要误判为不支持的异常并在持续用户态负载下触发整个内核 panic。
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
                crate::platform::report_trap("user-adem-unemulated", cx);
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
            crate::platform::report_trap("user-unsupported-interrupt", cx);
            panic!(
                "[kernel] Unsupported interrupt: {:?}, era = {:#x}",
                interrupt, cx.era
            );
        }
        cause => {
            let badv = badv::read();
            crate::platform::report_trap("user-unsupported-trap", cx);
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
            if let Err(err) = memory_set.handle_page_fault(cause, va, Some(cx.get_sp())) {
                crate::platform::report_fault_in_failure(va, cx.get_sp(), err.as_ret());
                return false;
            }
            va += PAGE_SIZE;
        }
        true
    })
}

/// 模拟 LA264 在 UAL=0 时触发 ADEM 的整数非对齐访存。
///
/// 解码 2RI12（ld/st）、3R（ldx/stx）和 2RI14（ll/sc/ldptr/stptr）三组指令，
/// 对有符号立即数显式扩展，再逐字节完成小端读写和符号/零扩展。目标跨页时，调用者已先用
/// `fault_in_emulated_access` 确保每页权限和惰性/COW 状态正确；本函数不能绕过 MemorySet
/// 直接把未映射用户地址当作内核指针访问。
///
/// 成功后写回目标寄存器并把 ERA 推进一条指令；返回 false 表示编码不在安全模拟集合，
/// 由上层按真实地址错误处理。LL/SC 只能提供当前内核范围内的兼容模拟，不宣称跨核保留硬件 reservation。
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
        // 12 位有符号立即数：必须显式符号扩展。`as i16` 对 0xFFE 只会得到 +4094，
        // 负偏移（如 st.h $t1,$t0,-2）会被算到错误地址，偏一页打到栈 VMA 之外。
        let v12 = ((inst >> 10) & 0xfff) as isize;
        let si12 = if v12 & 0x800 != 0 { v12 - 0x1000 } else { v12 };
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
    // 无符号变体 ldx.bu/hu/wu 是 0x7040/0x7048/0x7050（LLVM LoongArchInstrInfo.td 的
    // LDX_BU/LDX_HU/LDX_WU），不是 0x7004/0x700c/0x7014。
    let opcode17 = (inst >> 15) & 0x1_ffff;
    if (0x7000..=0x7050).contains(&opcode17) {
        let rk = (inst >> 10) & 0x1f;
        let rj = (inst >> 5) & 0x1f;
        let rd = inst & 0x1f;
        let addr = cx.x[rj].wrapping_add(cx.x[rk]);
        let (size, is_store) = match opcode17 {
            0x7000 | 0x7040 => (1, false), // ldx.b / ldx.bu
            0x7008 | 0x7048 => (2, false), // ldx.h / ldx.hu
            0x7010 | 0x7050 => (4, false), // ldx.w / ldx.wu
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
                0x7040 => read_bytes(addr, 1),
                0x7008 => (read_bytes(addr, 2) as u16 as i16) as isize as usize,
                0x7048 => read_bytes(addr, 2),
                0x7010 => (read_bytes(addr, 4) as u32 as i32) as isize as usize,
                0x7050 => read_bytes(addr, 4),
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

    // 2RI14：ll/sc/ldptr/stptr。操作码是 8 位（inst[31:24] = 0x20..0x27，
    // 高 6 位 001000/001001、低 2 位是子操作码），si14=inst[23:10]。
    // 注意：2RI14 立即数是 simm14_lsl2 —— 按 4 字节缩放（如分支偏移），
    // 汇编器把「偏移 4」编码成字段值 1；不乘 4 会错位 3 字节（musl memcpy
    // 的 ldptr.w +4/+8/+12 就是这样被模拟错，进而写坏 zlib 码表、git 崩溃）。
    let opcode8 = (inst >> 24) & 0xff;
    if (0x20..=0x27).contains(&opcode8) {
        let v14 = ((inst >> 10) & 0x3fff) as isize;
        let si14_field = if v14 & 0x2000 != 0 { v14 - 0x4000 } else { v14 };
        let si14 = si14_field << 2;
        let rj = (inst >> 5) & 0x1f;
        let rd = inst & 0x1f;
        let addr = (cx.x[rj] as isize).wrapping_add(si14) as usize;
        let (size, is_store) = match opcode8 {
            0x20 => (4, false), // ll.w
            0x22 => (8, false), // ll.d
            0x24 => (4, false), // ldptr.w
            0x26 => (8, false), // ldptr.d
            0x21 => (4, true),  // sc.w
            0x23 => (8, true),  // sc.d
            0x25 => (4, true),  // stptr.w
            0x27 => (8, true),  // stptr.d
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
            if (opcode8 == 0x21 || opcode8 == 0x23) && rd != 0 {
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
/// LoongArch 内核态 trap 分派器。
///
/// 内核 ADEM 可在确认指令属于安全模拟集合后由 `emulate_unaligned_access` 恢复；真正页错误、
/// 非法指令和未知异常属于内核不变量破坏。timer/IPI 只应答硬件，并仅在 idle 安全点处理全局
/// 超时工作；不能在被中断的任意锁临界区调度用户任务或投递用户 signal。
pub fn trap_from_kernel(cx: &mut TrapContext) {
    match estat::cause(estat::read()) {
        estat::Trap::Exception(estat::Exception::Breakpoint) => {
            println!("[kernel] Breakpoint at 0x{:x}", cx.era);
            cx.era += 4; // LoongArch break 指令为 4 字节
        }
        estat::Trap::Exception(estat::Exception::IllegalInstruction) => {
            crate::platform::report_trap("kernel-illegal-instr", cx);
            panic!("[kernel] IllegalInstruction at 0x{:x}", cx.era);
        }
        estat::Trap::Exception(estat::Exception::AddressNotAligned) => {
            // LA264 UAL=0，lwext4 C 库等代码的非对齐访存走这里逐字节模拟。
            if !emulate_unaligned_access(cx) {
                crate::platform::report_trap("kernel-adem-unemulated", cx);
                panic!(
                    "[kernel] AddressNotAligned not emulated: era = {:#x}, badv = {:#x}, inst = {:#x}",
                    cx.era,
                    badv::read(),
                    unsafe { core::ptr::read_volatile(cx.era as *const u32) }
                );
            }
        }
        estat::Trap::Exception(exception) if is_page_fault(exception) => {
            crate::platform::report_trap("kernel-page-fault", cx);
            panic!(
                "[kernel] page fault in kernel, era = {:#x}, badaddr = {:#x}, cause = {:?}",
                cx.era,
                badv::read(),
                estat::Trap::Exception(exception)
            );
        }
        estat::Trap::Exception(estat::Exception::Syscall) => {
            crate::platform::report_trap("kernel-syscall", cx);
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
            crate::platform::report_trap("kernel-unsupported-trap", cx);
            panic!(
                "[kernel] Unsupported trap in kernel: cause = {:?}, era = {:#x}, badv = {:#x}!",
                cause,
                cx.era,
                badv::read()
            );
        }
    }
}
