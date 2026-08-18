//! 控制终端、termios、输入队列和前台进程组语义。
//!
//! 当前 console TTY 把 SBI/UART 字节流转换为 canonical 或 noncanonical 用户输入，并实现
//! 常用 ioctl。输入处理必须区分字节编码与字符显示；内核 console 已按 UTF-8 字节输出，TTY
//! 不能再次按 Unicode scalar 重编码。
//!
//! session、controlling tty 与 foreground process group 是进程级关系。后台进程读写、控制
//! 字符生成 signal、`TIOCSCTTY/TIOCSPGRP` 权限和 orphan group 行为都依赖 ProcessManager，
//! 不能仅检查当前线程 PID。读取队列与 signal/进程状态分属不同锁域，阻塞读取要避免在持有
//! TTY 锁时投递 signal 或切换任务。

use crate::mm::{copy_from_user, copy_to_user};
use crate::mutex::SpinNoIrqLock;
use crate::sbi::console_getchar;
use crate::signal::{SiField, Sig, SigInfo};
use crate::syscall::{Errno, SysResult};
use crate::task::{PROCESS_MANAGER, ProcessLifecycle, current_task, yield_current_task};
use alloc::collections::VecDeque;
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
const IGNCR: u32 = 0x80;
const INLCR: u32 = 0x40;
const ICRNL: u32 = 0x100;
const ISTRIP: u32 = 0x20;
const OPOST: u32 = 0x1;
const ONLCR: u32 = 0x4;
const ISIG: u32 = 0x1;
const ICANON: u32 = 0x2;
const ECHO: u32 = 0x8;
const ECHOE: u32 = 0x10;
const ECHOK: u32 = 0x20;
const ECHONL: u32 = 0x40;
const NOFLSH: u32 = 0x80;
const TOSTOP: u32 = 0x100;
const ECHOCTL: u32 = 0x200;
const IEXTEN: u32 = 0x8000;

const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;
const VSUSP: usize = 10;
const VEOL: usize = 11;
const VEOL2: usize = 16;

const fn default_control_chars() -> [u8; 19] {
    let mut cc = [0; 19];
    cc[VINTR] = 3;
    cc[VQUIT] = 28;
    cc[VERASE] = 127;
    cc[VKILL] = 21;
    cc[VEOF] = 4;
    cc[VTIME] = 0;
    cc[VMIN] = 1;
    cc[8] = 17; // VSTART
    cc[9] = 19; // VSTOP
    cc[VSUSP] = 26;
    cc[12] = 18; // VREPRINT
    cc[13] = 15; // VDISCARD
    cc[14] = 23; // VWERASE
    cc[15] = 22; // VLNEXT
    cc
}

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
            lflag: 0x1 | 0x2 | 0x8 | 0x10 | 0x20 | 0x200 | 0x8000,
            line: 0,
            cc: default_control_chars(),
        }
    }
}

#[derive(Clone, Copy)]
struct TerminalState {
    session_id: usize,
    foreground_pgid: usize,
    termios: KernelTermios,
}

struct ConsoleInputState {
    edit: alloc::vec::Vec<u8>,
    ready: VecDeque<u8>,
    /// 每条规范模式记录尚未读取的字节数。零长度记录表示行首收到 VEOF。
    records: VecDeque<usize>,
}

impl ConsoleInputState {
    fn new() -> Self {
        Self {
            edit: alloc::vec::Vec::new(),
            ready: VecDeque::new(),
            records: VecDeque::new(),
        }
    }

    fn flush(&mut self) {
        self.edit.clear();
        self.ready.clear();
        self.records.clear();
    }

    fn commit_record(&mut self) {
        let len = self.edit.len();
        self.ready.extend(self.edit.drain(..));
        self.records.push_back(len);
    }
}

lazy_static! {
    /// RespOS 当前通过标准输入输出和 `/dev/tty` 暴露同一个物理控制台终端。
    /// 终端所有权放在这里，而不是 ioctl 或某个任意文件描述符中，
    /// 从而保证所有打开实例观察到相同的作业控制状态。
    static ref CONSOLE_TERMINAL: SpinNoIrqLock<TerminalState> =
        SpinNoIrqLock::new(TerminalState {
            session_id: 1,
            foreground_pgid: 1,
            termios: KernelTermios::console_default(),
        });
    static ref CONSOLE_INPUT: SpinNoIrqLock<ConsoleInputState> =
        SpinNoIrqLock::new(ConsoleInputState::new());
    /// 固件控制台读取会消费数据。定时服务核与前台读取者都可能抽取输入，
    /// 因此必须串行化整个排空操作，而不只是保护目标队列。
    static ref CONSOLE_PUMP: SpinNoIrqLock<()> = SpinNoIrqLock::new(());
}

#[derive(Clone, Copy)]
pub enum ConsoleReadKind {
    Stdin,
    DevTty,
}

fn record_read_yield(kind: ConsoleReadKind) {
    crate::perf::fs_yield(1);
    match kind {
        ConsoleReadKind::Stdin => crate::perf::stdio_yield(1),
        ConsoleReadKind::DevTty => crate::perf::tty_yield(1),
    }
}

fn control_char(termios: KernelTermios, index: usize, byte: u8) -> bool {
    termios.cc[index] != 0 && termios.cc[index] == byte
}

fn echo_control(byte: u8, termios: KernelTermios) {
    if termios.lflag & ECHO == 0 {
        return;
    }
    if termios.lflag & ECHOCTL != 0 && (byte < b' ' || byte == 0x7f) {
        let shown = if byte == 0x7f { b'?' } else { byte ^ 0x40 };
        crate::console::write_user_bytes(&[b'^', shown]);
    }
}

fn echo_byte(byte: u8, termios: KernelTermios) {
    if termios.lflag & ECHO != 0 || (byte == b'\n' && termios.lflag & ECHONL != 0) {
        if byte == b'\n' && termios.oflag & OPOST != 0 && termios.oflag & ONLCR != 0 {
            crate::console::write_user_bytes(b"\r\n");
        } else {
            crate::console::write_user_bytes(&[byte]);
        }
    }
}

fn terminal_signal(byte: u8, termios: KernelTermios) -> Option<Sig> {
    if termios.lflag & ISIG == 0 {
        return None;
    }
    if control_char(termios, VINTR, byte) {
        Some(Sig::SIGINT)
    } else if control_char(termios, VQUIT, byte) {
        Some(Sig::SIGQUIT)
    } else if control_char(termios, VSUSP, byte) {
        Some(Sig::SIGTSTP)
    } else {
        None
    }
}

/// 将固件控制台当前可用的全部字节送入共享线路规程处理。
///
/// 固件读取会消费字节，因此整个 drain 由 `CONSOLE_PUMP` 串行化，防止 timer-service 与前台
/// reader 各取走一半。每个字节依次应用输入转换、canonical 编辑、回显和 ISIG 控制字符；
/// 产生 VINTR/VQUIT/VSUSP 时向当前前台进程组发信号，即使此刻没有任务阻塞在 read。
fn pump_console_input() {
    let _pump = CONSOLE_PUMP.lock();
    let termios = CONSOLE_TERMINAL.lock().termios;
    let terminal = *CONSOLE_TERMINAL.lock();
    loop {
        let raw = console_getchar();
        if raw == 0 || raw > u8::MAX as usize {
            break;
        }
        let mut byte = raw as u8;
        if termios.iflag & ISTRIP != 0 {
            byte &= 0x7f;
        }
        if byte == b'\r' {
            if termios.iflag & IGNCR != 0 {
                continue;
            }
            if termios.iflag & ICRNL != 0 {
                byte = b'\n';
            }
        } else if byte == b'\n' && termios.iflag & INLCR != 0 {
            byte = b'\r';
        }

        if let Some(sig) = terminal_signal(byte, termios) {
            echo_control(byte, termios);
            if termios.lflag & NOFLSH == 0 {
                CONSOLE_INPUT.lock().flush();
            }
            signal_process_group(terminal.session_id, terminal.foreground_pgid, &[sig]);
            continue;
        }

        let mut input = CONSOLE_INPUT.lock();
        if termios.lflag & ICANON == 0 {
            input.ready.push_back(byte);
            drop(input);
            echo_byte(byte, termios);
            continue;
        }

        if control_char(termios, VERASE, byte) {
            if input.edit.pop().is_some() && termios.lflag & ECHO != 0 {
                if termios.lflag & ECHOE != 0 {
                    crate::console::write_user_bytes(b"\x08 \x08");
                } else {
                    echo_control(byte, termios);
                }
            }
            continue;
        }
        if control_char(termios, VKILL, byte) {
            input.edit.clear();
            drop(input);
            if termios.lflag & ECHO != 0 {
                if termios.lflag & ECHOK != 0 {
                    echo_byte(b'\n', termios);
                } else {
                    echo_control(byte, termios);
                }
            }
            continue;
        }
        if control_char(termios, VEOF, byte) {
            input.commit_record();
            continue;
        }

        let is_delimiter = byte == b'\n'
            || control_char(termios, VEOL, byte)
            || (termios.lflag & IEXTEN != 0 && control_char(termios, VEOL2, byte));
        input.edit.push(byte);
        if is_delimiter {
            input.commit_record();
        }
        drop(input);
        echo_byte(byte, termios);
    }
}

/// 在全局定时器安全点处理物理控制台输入。即使没有进程读取 tty，终端控制字符
/// 也必须产生信号，例如前台作业睡眠时输入 Ctrl-C。
pub fn poll_console_input() {
    pump_console_input();
}

fn drain_ready(buf: &mut [u8], canonical: bool) -> usize {
    let mut input = CONSOLE_INPUT.lock();
    let limit = if canonical {
        match input.records.front().copied() {
            Some(0) => {
                input.records.pop_front();
                return 0;
            }
            Some(len) => len.min(buf.len()),
            None => return 0,
        }
    } else {
        input.ready.len().min(buf.len())
    };
    for slot in &mut buf[..limit] {
        *slot = input.ready.pop_front().unwrap();
    }
    if canonical {
        let remaining = input.records.front_mut().unwrap();
        *remaining -= limit;
        if *remaining == 0 {
            input.records.pop_front();
        }
    }
    limit
}

pub fn console_read_ready() -> bool {
    pump_console_input();
    let termios = CONSOLE_TERMINAL.lock().termios;
    let input = CONSOLE_INPUT.lock();
    if termios.lflag & ICANON != 0 {
        !input.records.is_empty()
    } else {
        !input.ready.is_empty()
    }
}

pub fn console_available_bytes() -> usize {
    pump_console_input();
    CONSOLE_INPUT.lock().ready.len()
}

/// 按当前 termios 从共享控制台读取，并实现 canonical/raw 与作业控制阻塞语义。
///
/// 读取前执行后台进程组检查；canonical 模式只返回完整行、VEOF 记录或信号，raw 模式按
/// VMIN/VTIME 组合决定立即返回、最少字节和首字节后超时。无数据时先登记控制台 waiter，
/// 再 pump/复查并发布 Blocked，以关闭输入到达与睡眠之间的窗口。
///
/// 已读取字节构成 partial progress，后续信号/超时不能覆盖它；零进展被可递送信号打断才
/// 返回 EINTR。退出前必须撤销 waiter 和临时定时器。
pub fn read_console(buf: &mut [u8], kind: ConsoleReadKind) -> SysResult<usize> {
    check_background_read()?;
    if buf.is_empty() {
        return Ok(0);
    }
    let termios = CONSOLE_TERMINAL.lock().termios;
    let canonical = termios.lflag & ICANON != 0;
    if canonical {
        loop {
            pump_console_input();
            if current_task().is_some_and(|task| task.check_signal_interrupt()) {
                return Err(Errno::EINTR);
            }
            if !CONSOLE_INPUT.lock().records.is_empty() {
                return Ok(drain_ready(buf, true));
            }
            record_read_yield(kind);
            yield_current_task();
        }
    }

    let minimum = usize::from(termios.cc[VMIN]).min(buf.len());
    let timeout_us = usize::from(termios.cc[VTIME]).saturating_mul(100_000);
    let mut deadline = if minimum == 0 && timeout_us != 0 {
        Some(crate::arch::timer::get_timeout_us().saturating_add(timeout_us))
    } else {
        None
    };
    let mut last_available = 0usize;
    loop {
        pump_console_input();
        if current_task().is_some_and(|task| task.check_signal_interrupt()) {
            return Err(Errno::EINTR);
        }
        let available = CONSOLE_INPUT.lock().ready.len();
        if minimum == 0 && timeout_us == 0 {
            return Ok(drain_ready(buf, false));
        }
        if minimum == 0 && available != 0 {
            return Ok(drain_ready(buf, false));
        }
        if minimum != 0 && available >= minimum {
            return Ok(drain_ready(buf, false));
        }
        if minimum != 0 && timeout_us != 0 && available != 0 && available != last_available {
            deadline = Some(crate::arch::timer::get_timeout_us().saturating_add(timeout_us));
        }
        last_available = available;
        if deadline.is_some_and(|end| crate::arch::timer::get_timeout_us() >= end) {
            return Ok(drain_ready(buf, false));
        }
        record_read_yield(kind);
        yield_current_task();
    }
}

pub fn write_console(buf: &[u8]) {
    let termios = CONSOLE_TERMINAL.lock().termios;
    if termios.oflag & OPOST == 0 || termios.oflag & ONLCR == 0 {
        crate::console::write_user_bytes(buf);
        return;
    }
    let mut start = 0;
    for (index, &byte) in buf.iter().enumerate() {
        if byte != b'\n' {
            continue;
        }
        if start < index {
            crate::console::write_user_bytes(&buf[start..index]);
        }
        crate::console::write_user_bytes(b"\r\n");
        start = index + 1;
    }
    if start < buf.len() {
        crate::console::write_user_bytes(&buf[start..]);
    }
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

/// POSIX 要求新成为孤儿且包含停止进程的进程组依次收到 SIGHUP 与 SIGCONT。调用者先快照
/// `was_orphaned` before changing parent/session/group relations and invoke
/// 关系，并在关系提交后执行本操作。
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

/// 接触物理控制台前执行 POSIX 后台读取规则。SIGTTIN 被忽略/阻塞或进程组已孤儿化时
/// 返回 EIO；否则通过 SIGTTIN 停止整个后台进程组。
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

/// fd 层确认描述符指向 tty 后，分派 termios、窗口大小、会话和前台进程组 ioctl。
///
/// 所有 get 操作先快照内核状态再 copyout；set 操作先完整 copyin/校验，再一次提交，避免
/// EFAULT 改变半份终端状态。`TIOCSCTTY`、`TIOCNOTTY`、`TIOCSPGRP` 必须检查 session leader、
/// controlling tty 和同会话进程组关系；后台修改终端设置还需按 SIGTTOU 作业控制规则处理。
/// 未实现请求返回 ENOTTY，不能假成功欺骗 libc。
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
            if request == TCSETSF {
                CONSOLE_INPUT.lock().flush();
            }
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
