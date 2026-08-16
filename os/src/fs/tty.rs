use crate::mm::{copy_from_user, copy_to_user};
use crate::mutex::SpinNoIrqLock;
use crate::signal::{SiField, Sig, SigInfo};
use crate::syscall::{Errno, SysResult};
use crate::task::{PROCESS_MANAGER, ProcessLifecycle, current_task};
use lazy_static::lazy_static;

pub const TIOCSCTTY: usize = 0x540e;
pub const TIOCGPGRP: usize = 0x540f;
pub const TIOCSPGRP: usize = 0x5410;
pub const TCGETS: usize = 0x5401;
pub const TCSETS: usize = 0x5402;
pub const TCSETSW: usize = 0x5403;
pub const TCSETSF: usize = 0x5404;
pub const TIOCNOTTY: usize = 0x5422;
pub const TIOCGSID: usize = 0x5429;

const CAP_SYS_ADMIN: usize = 21;
const TOSTOP: u32 = 0x100;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KernelTermios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    line: u8,
    cc: [u8; 19],
}

impl KernelTermios {
    const fn console_default() -> Self {
        Self {
            iflag: 0x100,             // ICRNL
            oflag: 0x1 | 0x4,         // OPOST | ONLCR
            cflag: 0xf | 0x30 | 0x80, // B38400 | CS8 | CREAD
            lflag: 0x1 | 0x2 | 0x8 | 0x10 | 0x20 | 0x8000,
            line: 0,
            cc: [0; 19],
        }
    }
}

#[derive(Clone, Copy)]
struct TerminalState {
    session_id: usize,
    foreground_pgid: usize,
    termios: KernelTermios,
}

lazy_static! {
    /// RespOS currently exposes one physical console terminal through stdio
    /// and /dev/tty. Keep terminal ownership here rather than in ioctl or an
    /// arbitrary file descriptor, so all opens observe the same job-control
    /// state.
    static ref CONSOLE_TERMINAL: SpinNoIrqLock<TerminalState> =
        SpinNoIrqLock::new(TerminalState {
            session_id: 1,
            foreground_pgid: 1,
            termios: KernelTermios::console_default(),
        });
}

fn controls_console(process: &crate::task::ProcessState, terminal: TerminalState) -> bool {
    process.has_controlling_tty()
        && terminal.session_id != 0
        && process.sid() == terminal.session_id
}

fn process_group_exists(session_id: usize, pgid: usize) -> bool {
    let mut exists = false;
    PROCESS_MANAGER.for_each(|process| {
        if process.sid() == session_id
            && process.pgid() == pgid
            && !matches!(
                process.lifecycle(),
                ProcessLifecycle::Zombie | ProcessLifecycle::Reaped
            )
        {
            exists = true;
        }
    });
    exists
}

fn process_is_live(process: &crate::task::ProcessState) -> bool {
    matches!(
        process.lifecycle(),
        ProcessLifecycle::Running | ProcessLifecycle::Exec
    )
}

fn set_session_controlling_tty(session_id: usize, present: bool) {
    PROCESS_MANAGER.for_each(|process| {
        if process.sid() == session_id {
            process.set_controlling_tty(present);
        }
    });
}

fn signal_process_group(session_id: usize, pgid: usize, signals: &[Sig]) {
    if session_id == 0 || pgid == 0 {
        return;
    }
    let mut targets = alloc::vec::Vec::new();
    PROCESS_MANAGER.for_each(|process| {
        if process.sid() == session_id && process.pgid() == pgid && process_is_live(&process) {
            targets.push(process.clone());
        }
    });
    for process in targets {
        let Some(task) = process.signal_target() else {
            continue;
        };
        for &sig in signals {
            let siginfo = SigInfo::new(sig.raw(), SigInfo::KERNEL, SiField::None);
            task.receive_siginfo(siginfo, false);
            if sig == Sig::SIGCONT && task.is_stopped() {
                task.set_wait_event(SigInfo::CLD_CONTINUED, sig.raw());
                task.notify_parent_sigchld(SigInfo::CLD_CONTINUED);
                crate::task::wakeup_stopped_task(task.clone());
            }
        }
    }
}

fn signal_foreground_group(session_id: usize, pgid: usize) {
    signal_process_group(session_id, pgid, &[Sig::SIGHUP, Sig::SIGCONT]);
}

pub fn process_group_is_orphaned(session_id: usize, pgid: usize) -> bool {
    let mut orphaned = true;
    PROCESS_MANAGER.for_each(|process| {
        if process.sid() != session_id || process.pgid() != pgid || !process_is_live(&process) {
            return;
        }
        if let Some(parent) = process.parent() {
            if process_is_live(&parent) && parent.sid() == session_id && parent.pgid() != pgid {
                orphaned = false;
            }
        }
    });
    orphaned
}

/// POSIX requires a newly orphaned process group which contains stopped
/// processes to receive SIGHUP followed by SIGCONT. Callers snapshot
/// `was_orphaned` before changing parent/session/group relations and invoke
/// this after the relation is committed.
pub fn notify_orphaned_process_group_transition(
    session_id: usize,
    pgid: usize,
    was_orphaned: bool,
) {
    if session_id == 0 || pgid == 0 || was_orphaned || !process_group_is_orphaned(session_id, pgid)
    {
        return;
    }
    let mut has_stopped_member = false;
    PROCESS_MANAGER.for_each(|process| {
        if process.sid() == session_id
            && process.pgid() == pgid
            && process_is_live(&process)
            && process.members().iter().any(|task| task.is_stopped())
        {
            has_stopped_member = true;
        }
    });
    if has_stopped_member {
        signal_process_group(session_id, pgid, &[Sig::SIGHUP, Sig::SIGCONT]);
    }
}

/// Enforce the POSIX background-read rule before touching the physical
/// console. Ignored/blocked SIGTTIN and orphaned process groups get EIO;
/// otherwise the whole background group is stopped through SIGTTIN.
pub fn check_background_read() -> SysResult {
    let task = current_task().expect("[kernel] current task is None.");
    let process = task.process();
    let terminal = *CONSOLE_TERMINAL.lock();
    if !controls_console(&process, terminal) || process.pgid() == terminal.foreground_pgid {
        return Ok(());
    }

    let sig = Sig::SIGTTIN;
    let ignored = task.op_sig_handler(|handler| handler.get(sig).sa_handler == 1);
    let blocked = task.op_sig_pending(|pending| pending.mask.contain_signal(sig));
    if ignored || blocked || process_group_is_orphaned(process.sid(), process.pgid()) {
        return Err(Errno::EIO);
    }
    signal_process_group(process.sid(), process.pgid(), &[sig]);
    Err(Errno::EINTR)
}

pub fn check_background_write() -> SysResult {
    let task = current_task().expect("[kernel] current task is None.");
    let process = task.process();
    let terminal = *CONSOLE_TERMINAL.lock();
    if !controls_console(&process, terminal)
        || process.pgid() == terminal.foreground_pgid
        || terminal.termios.lflag & TOSTOP == 0
    {
        return Ok(());
    }

    let sig = Sig::SIGTTOU;
    let ignored = task.op_sig_handler(|handler| handler.get(sig).sa_handler == 1);
    let blocked = task.op_sig_pending(|pending| pending.mask.contain_signal(sig));
    if ignored || blocked {
        return Ok(());
    }
    if process_group_is_orphaned(process.sid(), process.pgid()) {
        return Err(Errno::EIO);
    }
    signal_process_group(process.sid(), process.pgid(), &[sig]);
    Err(Errno::EINTR)
}

fn check_background_terminal_change() -> SysResult {
    let task = current_task().expect("[kernel] current task is None.");
    let process = task.process();
    let terminal = *CONSOLE_TERMINAL.lock();
    if !controls_console(&process, terminal) || process.pgid() == terminal.foreground_pgid {
        return Ok(());
    }

    let sig = Sig::SIGTTOU;
    let ignored = task.op_sig_handler(|handler| handler.get(sig).sa_handler == 1);
    let blocked = task.op_sig_pending(|pending| pending.mask.contain_signal(sig));
    if ignored || blocked {
        return Ok(());
    }
    if process_group_is_orphaned(process.sid(), process.pgid()) {
        return Err(Errno::EIO);
    }
    signal_process_group(process.sid(), process.pgid(), &[sig]);
    Err(Errno::EINTR)
}

/// Dispatch console-terminal ioctls after the fd layer has established that
/// the descriptor refers to a tty.
pub fn console_ioctl(request: usize, arg: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let process = task.process();

    match request {
        TCGETS => {
            let termios = CONSOLE_TERMINAL.lock().termios;
            copy_to_user(
                arg as *mut KernelTermios,
                &termios as *const KernelTermios,
                1,
            )?;
            Ok(0)
        }
        TCSETS | TCSETSW | TCSETSF => {
            check_background_terminal_change()?;
            let mut termios = KernelTermios::console_default();
            copy_from_user(
                &mut termios as *mut KernelTermios,
                arg as *const KernelTermios,
                1,
            )?;
            CONSOLE_TERMINAL.lock().termios = termios;
            Ok(0)
        }
        TIOCGSID => {
            let terminal = *CONSOLE_TERMINAL.lock();
            if !controls_console(&process, terminal) {
                return Err(Errno::ENOTTY);
            }
            let sid = terminal.session_id as i32;
            copy_to_user(arg as *mut i32, &sid as *const i32, 1)?;
            Ok(0)
        }
        TIOCGPGRP => {
            let terminal = *CONSOLE_TERMINAL.lock();
            if !controls_console(&process, terminal) {
                return Err(Errno::ENOTTY);
            }
            let pgid = terminal.foreground_pgid as i32;
            copy_to_user(arg as *mut i32, &pgid as *const i32, 1)?;
            Ok(0)
        }
        TIOCSPGRP => {
            let mut pgid = 0i32;
            copy_from_user(&mut pgid as *mut i32, arg as *const i32, 1)?;
            if pgid <= 0 {
                return Err(Errno::EINVAL);
            }

            check_background_terminal_change()?;

            let terminal_snapshot = *CONSOLE_TERMINAL.lock();
            if !controls_console(&process, terminal_snapshot) {
                return Err(Errno::ENOTTY);
            }
            if !process_group_exists(terminal_snapshot.session_id, pgid as usize) {
                return Err(Errno::ESRCH);
            }
            let mut terminal = CONSOLE_TERMINAL.lock();
            if !controls_console(&process, *terminal)
                || terminal.session_id != terminal_snapshot.session_id
            {
                return Err(Errno::ENOTTY);
            }
            terminal.foreground_pgid = pgid as usize;
            Ok(0)
        }
        TIOCSCTTY => {
            if process.sid() != process.tgid() {
                return Err(Errno::EPERM);
            }

            let mut terminal = CONSOLE_TERMINAL.lock();
            if controls_console(&process, *terminal) {
                return Ok(0);
            }
            if terminal.session_id != 0 && terminal.session_id != process.sid() {
                if arg != 1 || !task.has_cap(CAP_SYS_ADMIN) {
                    return Err(Errno::EPERM);
                }
            }
            let old_session = terminal.session_id;
            terminal.session_id = process.sid();
            terminal.foreground_pgid = process.pgid();
            drop(terminal);

            if old_session != 0 && old_session != process.sid() {
                set_session_controlling_tty(old_session, false);
            }
            process.set_controlling_tty(true);
            Ok(0)
        }
        TIOCNOTTY => {
            let mut terminal = CONSOLE_TERMINAL.lock();
            if !controls_console(&process, *terminal) {
                return Err(Errno::ENOTTY);
            }

            if process.sid() == process.tgid() {
                let old_session = terminal.session_id;
                let old_foreground = terminal.foreground_pgid;
                terminal.session_id = 0;
                terminal.foreground_pgid = 0;
                drop(terminal);
                set_session_controlling_tty(old_session, false);
                signal_foreground_group(old_session, old_foreground);
            } else {
                process.set_controlling_tty(false);
            }
            Ok(0)
        }
        _ => Err(Errno::ENOTTY),
    }
}

pub fn detach_process_from_console(process: &crate::task::ProcessState) {
    process.set_controlling_tty(false);
}

pub fn release_console_on_session_exit(process: &crate::task::ProcessState) {
    if process.sid() != process.tgid() || !process.has_controlling_tty() {
        return;
    }
    let mut terminal = CONSOLE_TERMINAL.lock();
    if terminal.session_id != process.sid() {
        process.set_controlling_tty(false);
        return;
    }
    let old_session = terminal.session_id;
    let old_foreground = terminal.foreground_pgid;
    terminal.session_id = 0;
    terminal.foreground_pgid = 0;
    drop(terminal);
    set_session_controlling_tty(old_session, false);
    signal_foreground_group(old_session, old_foreground);
}
