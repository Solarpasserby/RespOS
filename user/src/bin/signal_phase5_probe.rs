#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    clock_gettime_raw, clock_nanosleep_raw, close, epoll_create1, epoll_ctl, epoll_pwait, exec,
    exit_group, fcntl, fork, futex_raw, getpid, getrusage_raw, kill, mmap_raw, munmap, nanosleep,
    pipe, ppoll_raw, prlimit64_raw, pselect6_raw, read, readv, sigaction_raw, sigpending_raw,
    sigprocmask_raw, sigqueueinfo_raw, sigtimedwait_raw, time_get, wait4_raw, write, writev,
    yield_, IoVec, PollFd, RLimit, RUsage, SignalAction, TimeSpec, O_NONBLOCK, SIGCHLD, SIGCONT,
    SIGKILL, SIGSTOP,
};

const SIG_BLOCK: usize = 0;
const SIGUSR1: i32 = 10;
const SIGUSR2: i32 = 12;
const SIGRTMIN: i32 = 34;
const SIGWINCH: i32 = 28;
const SIGSET_SIZE: usize = core::mem::size_of::<u64>();
const SI_QUEUE: i32 = -1;
const EFAULT: isize = 14;
const EINVAL: isize = 22;
const ECHILD: isize = 10;
const EINTR: isize = 4;
const RLIMIT_SIGPENDING: usize = 11;
const SA_NOCLDWAIT: u32 = 2;
const SA_NOCLDSTOP: u32 = 1;
const SA_RESTART: u32 = 0x1000_0000;
const WUNTRACED: usize = 1 << 1;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const PAGE_SIZE: usize = 4096;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;
static SIGCHLD_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGUSR1_COUNT: AtomicUsize = AtomicUsize::new(0);

fn count_sigchld() {
    SIGCHLD_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn count_sigusr1() {
    SIGUSR1_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[repr(C)]
struct LinuxSigInfo {
    signo: i32,
    errno: i32,
    code: i32,
    pad: [i32; 29],
}

impl LinuxSigInfo {
    fn queued(signo: i32, pid: i32, value: i32) -> Self {
        let mut info = Self {
            signo,
            errno: 0,
            code: SI_QUEUE,
            pad: [0; 29],
        };
        info.pad[1] = pid;
        info.pad[2] = 0;
        info.pad[3] = value;
        info
    }

    fn queued_value(&self) -> i32 {
        self.pad[3]
    }
}

fn signal_bit(signo: i32) -> u64 {
    1u64 << (signo - 1)
}

fn test_query_ignores_how() -> bool {
    let mut oldset = u64::MAX;
    let result = sigprocmask_raw(usize::MAX, null(), &mut oldset, SIGSET_SIZE);
    if result != 0 {
        println!(
            "SIGNAL_PHASE5_FAIL sigprocmask_query result={} oldset={:#x}",
            result, oldset
        );
        return false;
    }
    println!("SIGNAL_PHASE5 sigprocmask_query PASS");
    true
}

fn test_sigqueueinfo_null_signal() -> bool {
    let pid = getpid();
    if pid <= 0 {
        println!("SIGNAL_PHASE5_FAIL getpid result={}", pid);
        return false;
    }
    let info = LinuxSigInfo::queued(0, pid as i32, 0);
    let result = sigqueueinfo_raw(pid as usize, 0, &info as *const LinuxSigInfo as *const u8);
    if result != 0 {
        println!("SIGNAL_PHASE5_FAIL sigqueueinfo_zero result={}", result);
        return false;
    }
    println!("SIGNAL_PHASE5 sigqueueinfo_zero PASS");
    true
}

fn queue_signal(pid: usize, signo: i32, value: i32) -> bool {
    let info = LinuxSigInfo::queued(signo, pid as i32, value);
    sigqueueinfo_raw(pid, signo, &info as *const LinuxSigInfo as *const u8) == 0
}

fn wait_signal(signo: i32) -> Result<LinuxSigInfo, isize> {
    let set = signal_bit(signo);
    let timeout = TimeSpec { sec: 0, nsec: 0 };
    let mut info = LinuxSigInfo::queued(0, 0, 0);
    let result = sigtimedwait_raw(
        &set,
        &mut info as *mut LinuxSigInfo as *mut u8,
        &timeout,
        SIGSET_SIZE,
    );
    if result == signo as isize {
        Ok(info)
    } else {
        Err(result)
    }
}

fn test_pending_queue_semantics() -> bool {
    let pid = getpid();
    if pid <= 0 {
        return false;
    }
    let pid = pid as usize;
    let block = signal_bit(SIGUSR2) | signal_bit(SIGRTMIN);
    if sigprocmask_raw(SIG_BLOCK, &block, null_mut(), SIGSET_SIZE) != 0 {
        return false;
    }

    if !queue_signal(pid, SIGUSR2, 11) || !queue_signal(pid, SIGUSR2, 22) {
        return false;
    }
    let standard = match wait_signal(SIGUSR2) {
        Ok(info) => info,
        Err(result) => {
            println!("SIGNAL_PHASE5_FAIL standard_wait result={}", result);
            return false;
        }
    };
    if standard.code != SI_QUEUE
        || standard.queued_value() != 11
        || wait_signal(SIGUSR2).err() != Some(-11)
    {
        println!(
            "SIGNAL_PHASE5_FAIL standard_coalesce code={} value={}",
            standard.code,
            standard.queued_value()
        );
        return false;
    }

    for value in [101, 202, 303] {
        if !queue_signal(pid, SIGRTMIN, value) {
            return false;
        }
    }
    for expected in [101, 202, 303] {
        let info = match wait_signal(SIGRTMIN) {
            Ok(info) => info,
            Err(result) => {
                println!("SIGNAL_PHASE5_FAIL realtime_wait result={}", result);
                return false;
            }
        };
        if info.signo != SIGRTMIN || info.code != SI_QUEUE || info.queued_value() != expected {
            println!(
                "SIGNAL_PHASE5_FAIL realtime_fifo signo={} code={} value={} expected={}",
                info.signo,
                info.code,
                info.queued_value(),
                expected
            );
            return false;
        }
    }
    if wait_signal(SIGRTMIN).err() != Some(-11) {
        println!("SIGNAL_PHASE5_FAIL realtime_not_empty");
        return false;
    }

    let mut old_limit = RLimit::default();
    if prlimit64_raw(0, RLIMIT_SIGPENDING, null(), &mut old_limit) != 0 {
        return false;
    }
    let limited = RLimit {
        cur: 2,
        max: old_limit.max,
    };
    if prlimit64_raw(0, RLIMIT_SIGPENDING, &limited, null_mut()) != 0
        || !queue_signal(pid, SIGRTMIN, 401)
        || !queue_signal(pid, SIGRTMIN, 402)
        || queue_signal(pid, SIGRTMIN, 403)
    {
        println!("SIGNAL_PHASE5_FAIL realtime_limit_fill");
        return false;
    }
    if wait_signal(SIGRTMIN).map(|info| info.queued_value()) != Ok(401)
        || !queue_signal(pid, SIGRTMIN, 403)
        || wait_signal(SIGRTMIN).map(|info| info.queued_value()) != Ok(402)
        || wait_signal(SIGRTMIN).map(|info| info.queued_value()) != Ok(403)
    {
        println!("SIGNAL_PHASE5_FAIL realtime_limit_recovery");
        return false;
    }
    if prlimit64_raw(0, RLIMIT_SIGPENDING, &old_limit, null_mut()) != 0 {
        return false;
    }
    println!("SIGNAL_PHASE5 pending_queue_semantics PASS");
    true
}

fn test_sigaction_validation() -> bool {
    if sigaction_raw(SIGUSR1, null(), null_mut(), 0) != -EINVAL {
        println!("SIGNAL_PHASE5_FAIL sigaction_size");
        return false;
    }

    let mut old_action = SignalAction {
        handler: 0x1234,
        flags: 0x5678,
        restorer: 0x9abc,
        mask: 0xdef0,
    };
    let invalid_action = usize::MAX as *const SignalAction;
    let result = sigaction_raw(SIGUSR1, invalid_action, &mut old_action, SIGSET_SIZE);
    if result != -EFAULT
        || old_action.handler != 0x1234
        || old_action.flags != 0x5678
        || old_action.restorer != 0x9abc
        || old_action.mask != 0xdef0
    {
        println!(
            "SIGNAL_PHASE5_FAIL sigaction_input result={} old=({:#x},{:#x},{:#x},{:#x})",
            result, old_action.handler, old_action.flags, old_action.restorer, old_action.mask
        );
        return false;
    }
    println!("SIGNAL_PHASE5 sigaction_validation PASS");
    true
}

fn test_sigchld_autoreap() -> bool {
    let mut status = -1;
    let ignore = SignalAction {
        handler: 1,
        ..SignalAction::default()
    };
    if sigaction_raw(SIGCHLD, &ignore, null_mut(), SIGSET_SIZE) != 0 {
        return false;
    }
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = exit_group(0);
    }
    if wait4_raw(child, &mut status, 0, null_mut()) != -ECHILD {
        println!("SIGNAL_PHASE5_FAIL sigchld_ignore child={}", child);
        return false;
    }

    SIGCHLD_COUNT.store(0, Ordering::SeqCst);
    let no_cldwait = SignalAction {
        handler: count_sigchld as usize,
        flags: SA_NOCLDWAIT,
        ..SignalAction::default()
    };
    if sigaction_raw(SIGCHLD, &no_cldwait, null_mut(), SIGSET_SIZE) != 0 {
        return false;
    }
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = exit_group(0);
    }
    let no_cldwait_result = wait4_raw(child, &mut status, 0, null_mut());
    if no_cldwait_result != -ECHILD || SIGCHLD_COUNT.load(Ordering::SeqCst) != 1 {
        println!(
            "SIGNAL_PHASE5_FAIL sigchld_nocldwait child={} result={} count={}",
            child,
            no_cldwait_result,
            SIGCHLD_COUNT.load(Ordering::SeqCst)
        );
        return false;
    }

    let default = SignalAction::default();
    if sigaction_raw(SIGCHLD, &default, null_mut(), SIGSET_SIZE) != 0 {
        return false;
    }
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = exit_group(0);
    }
    if wait4_raw(child, &mut status, 0, null_mut()) != child || status != 0 {
        println!(
            "SIGNAL_PHASE5_FAIL sigchld_default child={} status={}",
            child, status
        );
        return false;
    }
    println!("SIGNAL_PHASE5 sigchld_autoreap PASS");
    true
}

fn test_sigchld_nocldstop() -> bool {
    SIGCHLD_COUNT.store(0, Ordering::SeqCst);
    let no_cldstop = SignalAction {
        handler: count_sigchld as usize,
        flags: SA_NOCLDSTOP,
        ..SignalAction::default()
    };
    if sigaction_raw(SIGCHLD, &no_cldstop, null_mut(), SIGSET_SIZE) != 0 {
        return false;
    }
    let mut ready_pipe = [0i32; 2];
    if pipe(&mut ready_pipe) != 0 {
        return false;
    }
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = close(ready_pipe[0] as usize);
        let _ = write(ready_pipe[1] as usize, &[1]);
        let _ = close(ready_pipe[1] as usize);
        loop {
            let _ = yield_();
        }
    }
    let _ = close(ready_pipe[1] as usize);
    let mut ready = [0u8; 1];
    let ready_result = read(ready_pipe[0] as usize, &mut ready);
    let _ = close(ready_pipe[0] as usize);
    if ready_result != 1 || ready[0] != 1 {
        return false;
    }
    let mut status = 0;
    let stop_result = kill(child as usize, SIGSTOP);
    let stopped = wait4_raw(child, &mut status, WUNTRACED, null_mut());
    if stop_result != 0
        || stopped != child
        || status != (SIGSTOP << 8) | 0x7f
        || SIGCHLD_COUNT.load(Ordering::SeqCst) != 0
    {
        println!(
            "SIGNAL_PHASE5_FAIL sigchld_nocldstop child={} status={:#x} count={}",
            child,
            status,
            SIGCHLD_COUNT.load(Ordering::SeqCst)
        );
        return false;
    }
    if kill(child as usize, SIGCONT) != 0 || kill(child as usize, SIGKILL) != 0 {
        return false;
    }
    let reaped = wait4_raw(child, &mut status, 0, null_mut());
    if reaped != child {
        return false;
    }
    if sigaction_raw(SIGCHLD, &SignalAction::default(), null_mut(), SIGSET_SIZE) != 0 {
        return false;
    }
    println!("SIGNAL_PHASE5 sigchld_nocldstop PASS");
    true
}

fn run_pipe_read_signal_case(restart: bool, default_ignored: bool) -> bool {
    let signo = if default_ignored { SIGWINCH } else { SIGUSR1 };
    if !default_ignored {
        SIGUSR1_COUNT.store(0, Ordering::SeqCst);
        let action = SignalAction {
            handler: count_sigusr1 as usize,
            flags: if restart { SA_RESTART } else { 0 },
            ..SignalAction::default()
        };
        if sigaction_raw(signo, &action, null_mut(), SIGSET_SIZE) != 0 {
            return false;
        }
    }

    let mut fds = [0i32; 2];
    if pipe(&mut fds) != 0 {
        return false;
    }
    let parent = getpid();
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = close(fds[0] as usize);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let _ = kill(parent as usize, signo);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let _ = write(fds[1] as usize, &[0x5a]);
        let _ = close(fds[1] as usize);
        let _ = exit_group(0);
    }

    let _ = close(fds[1] as usize);
    let mut byte = [0u8; 1];
    let first = read(fds[0] as usize, &mut byte);
    let result_ok = if !restart && !default_ignored {
        first == -4 && read(fds[0] as usize, &mut byte) == 1 && byte[0] == 0x5a
    } else {
        first == 1 && byte[0] == 0x5a
    };
    let _ = close(fds[0] as usize);
    let mut status = -1;
    let waited = wait4_raw(child, &mut status, 0, null_mut());
    let count_ok = default_ignored || SIGUSR1_COUNT.load(Ordering::SeqCst) == 1;
    if !default_ignored {
        let _ = sigaction_raw(signo, &SignalAction::default(), null_mut(), SIGSET_SIZE);
    }
    if !result_ok || waited != child || status != 0 || !count_ok {
        println!(
            "SIGNAL_PHASE5_FAIL pipe_read_restart restart={} ignored={} first={} byte={} waited={} status={} count={}",
            restart,
            default_ignored,
            first,
            byte[0],
            waited,
            status,
            SIGUSR1_COUNT.load(Ordering::SeqCst)
        );
        return false;
    }
    true
}

fn test_pipe_read_restart() -> bool {
    if !run_pipe_read_signal_case(false, false)
        || !run_pipe_read_signal_case(true, false)
        || !run_pipe_read_signal_case(false, true)
    {
        return false;
    }
    println!("SIGNAL_PHASE5 pipe_read_restart PASS");
    true
}

fn fill_pipe(fd: usize) -> bool {
    let flags = fcntl(fd, F_GETFL, 0);
    if flags < 0 || fcntl(fd, F_SETFL, flags as usize | O_NONBLOCK) != 0 {
        return false;
    }
    let chunk = [0xa5u8; 4096];
    let mut total = 0usize;
    loop {
        let result = write(fd, &chunk);
        if result > 0 {
            total += result as usize;
        } else if result == -11 {
            break;
        } else {
            return false;
        }
    }
    fcntl(fd, F_SETFL, flags as usize) == 0 && total != 0
}

fn run_pipe_write_signal_case(restart: bool, default_ignored: bool) -> bool {
    let signo = if default_ignored { SIGWINCH } else { SIGUSR1 };
    if !default_ignored {
        SIGUSR1_COUNT.store(0, Ordering::SeqCst);
        let action = SignalAction {
            handler: count_sigusr1 as usize,
            flags: if restart { SA_RESTART } else { 0 },
            ..SignalAction::default()
        };
        if sigaction_raw(signo, &action, null_mut(), SIGSET_SIZE) != 0 {
            return false;
        }
    }

    let mut fds = [0i32; 2];
    let mut ack = [0i32; 2];
    if pipe(&mut fds) != 0 || pipe(&mut ack) != 0 || !fill_pipe(fds[1] as usize) {
        return false;
    }
    let parent = getpid();
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = close(fds[1] as usize);
        let _ = close(ack[1] as usize);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let _ = kill(parent as usize, signo);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let mut drained = [0u8; 4096];
        let result = read(fds[0] as usize, &mut drained);
        let ack_result = read(ack[0] as usize, &mut drained[..1]);
        let _ = close(fds[0] as usize);
        let _ = close(ack[0] as usize);
        let _ = exit_group(if result == 4096 && ack_result == 1 {
            0
        } else {
            1
        });
    }

    let _ = close(fds[0] as usize);
    let _ = close(ack[0] as usize);
    let first = write(fds[1] as usize, &[0x5a]);
    let result_ok = if !restart && !default_ignored {
        first == -4 && write(fds[1] as usize, &[0x5a]) == 1
    } else {
        first == 1
    };
    let ack_result = write(ack[1] as usize, &[1]);
    let _ = close(fds[1] as usize);
    let _ = close(ack[1] as usize);
    let mut status = -1;
    let waited = wait4_raw(child, &mut status, 0, null_mut());
    let count_ok = default_ignored || SIGUSR1_COUNT.load(Ordering::SeqCst) == 1;
    if !default_ignored {
        let _ = sigaction_raw(signo, &SignalAction::default(), null_mut(), SIGSET_SIZE);
    }
    if !result_ok || ack_result != 1 || waited != child || status != 0 || !count_ok {
        println!(
            "SIGNAL_PHASE5_FAIL pipe_write_restart restart={} ignored={} first={} ack={} waited={} status={} count={}",
            restart,
            default_ignored,
            first,
            ack_result,
            waited,
            status,
            SIGUSR1_COUNT.load(Ordering::SeqCst)
        );
        return false;
    }
    true
}

fn test_pipe_write_restart() -> bool {
    if !run_pipe_write_signal_case(false, false)
        || !run_pipe_write_signal_case(true, false)
        || !run_pipe_write_signal_case(false, true)
    {
        return false;
    }
    println!("SIGNAL_PHASE5 pipe_write_restart PASS");
    true
}

fn run_pipe_readv_signal_case(restart: bool, default_ignored: bool) -> bool {
    let signo = if default_ignored { SIGWINCH } else { SIGUSR1 };
    if !default_ignored {
        SIGUSR1_COUNT.store(0, Ordering::SeqCst);
        let action = SignalAction {
            handler: count_sigusr1 as usize,
            flags: if restart { SA_RESTART } else { 0 },
            ..SignalAction::default()
        };
        if sigaction_raw(signo, &action, null_mut(), SIGSET_SIZE) != 0 {
            return false;
        }
    }

    let mut fds = [0i32; 2];
    if pipe(&mut fds) != 0 {
        return false;
    }
    let parent = getpid();
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = close(fds[0] as usize);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let _ = kill(parent as usize, signo);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let _ = write(fds[1] as usize, &[0x6b]);
        let _ = close(fds[1] as usize);
        let _ = exit_group(0);
    }

    let _ = close(fds[1] as usize);
    let mut first_byte = [0u8; 1];
    let mut second_byte = [0u8; 1];
    let iov = [
        IoVec {
            base: first_byte.as_mut_ptr(),
            len: 1,
        },
        IoVec {
            base: second_byte.as_mut_ptr(),
            len: 1,
        },
    ];
    let first = readv(fds[0] as usize, &iov);
    let result = if !restart && !default_ignored {
        if first != -4 {
            first
        } else {
            readv(fds[0] as usize, &iov)
        }
    } else {
        first
    };
    let _ = close(fds[0] as usize);
    let mut status = -1;
    let waited = wait4_raw(child, &mut status, 0, null_mut());
    let count_ok = default_ignored || SIGUSR1_COUNT.load(Ordering::SeqCst) == 1;
    if !default_ignored {
        let _ = sigaction_raw(signo, &SignalAction::default(), null_mut(), SIGSET_SIZE);
    }
    if result != 1
        || first_byte[0] != 0x6b
        || second_byte[0] != 0
        || waited != child
        || status != 0
        || !count_ok
    {
        println!(
            "SIGNAL_PHASE5_FAIL pipe_readv_restart restart={} ignored={} first={} result={} bytes=({},{}) waited={} status={} count={}",
            restart,
            default_ignored,
            first,
            result,
            first_byte[0],
            second_byte[0],
            waited,
            status,
            SIGUSR1_COUNT.load(Ordering::SeqCst)
        );
        return false;
    }
    true
}

fn run_pipe_writev_signal_case(restart: bool, default_ignored: bool) -> bool {
    let signo = if default_ignored { SIGWINCH } else { SIGUSR1 };
    if !default_ignored {
        SIGUSR1_COUNT.store(0, Ordering::SeqCst);
        let action = SignalAction {
            handler: count_sigusr1 as usize,
            flags: if restart { SA_RESTART } else { 0 },
            ..SignalAction::default()
        };
        if sigaction_raw(signo, &action, null_mut(), SIGSET_SIZE) != 0 {
            return false;
        }
    }

    let mut fds = [0i32; 2];
    let mut ack = [0i32; 2];
    if pipe(&mut fds) != 0 || pipe(&mut ack) != 0 || !fill_pipe(fds[1] as usize) {
        return false;
    }
    let parent = getpid();
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let _ = close(fds[1] as usize);
        let _ = close(ack[1] as usize);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let _ = kill(parent as usize, signo);
        for _ in 0..4096 {
            let _ = yield_();
        }
        let mut drained = [0u8; 4096];
        let result = read(fds[0] as usize, &mut drained);
        let ack_result = read(ack[0] as usize, &mut drained[..1]);
        let _ = exit_group(if result == 4096 && ack_result == 1 {
            0
        } else {
            1
        });
    }

    let _ = close(fds[0] as usize);
    let _ = close(ack[0] as usize);
    let first_byte = [0x71u8; 1];
    let second_byte = [0x72u8; 1];
    let iov = [
        IoVec {
            base: first_byte.as_ptr() as *mut u8,
            len: 1,
        },
        IoVec {
            base: second_byte.as_ptr() as *mut u8,
            len: 1,
        },
    ];
    let first = writev(fds[1] as usize, &iov);
    let result = if !restart && !default_ignored {
        if first != -4 {
            first
        } else {
            writev(fds[1] as usize, &iov)
        }
    } else {
        first
    };
    let ack_result = write(ack[1] as usize, &[1]);
    let _ = close(fds[1] as usize);
    let _ = close(ack[1] as usize);
    let mut status = -1;
    let waited = wait4_raw(child, &mut status, 0, null_mut());
    let count_ok = default_ignored || SIGUSR1_COUNT.load(Ordering::SeqCst) == 1;
    if !default_ignored {
        let _ = sigaction_raw(signo, &SignalAction::default(), null_mut(), SIGSET_SIZE);
    }
    if result != 2 || ack_result != 1 || waited != child || status != 0 || !count_ok {
        println!(
            "SIGNAL_PHASE5_FAIL pipe_writev_restart restart={} ignored={} first={} result={} ack={} waited={} status={} count={}",
            restart,
            default_ignored,
            first,
            result,
            ack_result,
            waited,
            status,
            SIGUSR1_COUNT.load(Ordering::SeqCst)
        );
        return false;
    }
    true
}

fn test_pipe_vectored_restart() -> bool {
    for (restart, default_ignored) in [(false, false), (true, false), (false, true)] {
        if !run_pipe_readv_signal_case(restart, default_ignored)
            || !run_pipe_writev_signal_case(restart, default_ignored)
        {
            return false;
        }
    }
    println!("SIGNAL_PHASE5 pipe_vectored_restart PASS");
    true
}

fn run_futex_signal_case(restart: bool, default_ignored: bool) -> bool {
    let signo = if default_ignored { SIGWINCH } else { SIGUSR1 };
    if !default_ignored {
        SIGUSR1_COUNT.store(0, Ordering::SeqCst);
        let action = SignalAction {
            handler: count_sigusr1 as usize,
            flags: if restart { SA_RESTART } else { 0 },
            ..SignalAction::default()
        };
        if sigaction_raw(signo, &action, null_mut(), SIGSET_SIZE) != 0 {
            return false;
        }
    }

    let mapped = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    if mapped <= 0 {
        return false;
    }
    let futex = mapped as *mut u32;
    unsafe { futex.write(0) };
    let parent = getpid();
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        for _ in 0..4096 {
            let _ = yield_();
        }
        let _ = kill(parent as usize, signo);
        if restart || default_ignored {
            for _ in 0..4096 {
                let _ = yield_();
            }
            let woke = futex_raw(futex, FUTEX_WAKE, 1, null());
            exit_group(if woke == 1 { 0 } else { 91 });
        }
        exit_group(0);
    }

    let result = futex_raw(futex, FUTEX_WAIT, 0, null());
    let mut status = -1;
    let waited = wait4_raw(child, &mut status, 0, null_mut());
    let expected = if restart || default_ignored {
        0
    } else {
        -EINTR
    };
    let count_ok = default_ignored || SIGUSR1_COUNT.load(Ordering::SeqCst) == 1;
    if !default_ignored {
        let _ = sigaction_raw(signo, &SignalAction::default(), null_mut(), SIGSET_SIZE);
    }
    let unmapped = munmap(mapped as usize, PAGE_SIZE);
    if result != expected || waited != child || status != 0 || !count_ok || unmapped != 0 {
        println!(
            "SIGNAL_PHASE5_FAIL futex_restart restart={} ignored={} result={} waited={} status={} count={} munmap={}",
            restart,
            default_ignored,
            result,
            waited,
            status,
            SIGUSR1_COUNT.load(Ordering::SeqCst),
            unmapped
        );
        return false;
    }
    true
}

fn test_futex_restart() -> bool {
    if !run_futex_signal_case(false, false)
        || !run_futex_signal_case(true, false)
        || !run_futex_signal_case(false, true)
    {
        return false;
    }
    println!("SIGNAL_PHASE5 futex_restart PASS");
    true
}

fn run_timeout_signal_case(wait_kind: usize, restart: bool, default_ignored: bool) -> bool {
    let signo = if default_ignored { SIGWINCH } else { SIGUSR1 };
    if !default_ignored {
        SIGUSR1_COUNT.store(0, Ordering::SeqCst);
        let action = SignalAction {
            handler: count_sigusr1 as usize,
            flags: if restart { SA_RESTART } else { 0 },
            ..SignalAction::default()
        };
        if sigaction_raw(signo, &action, null_mut(), SIGSET_SIZE) != 0 {
            return false;
        }
    }

    let parent = getpid();
    let child = fork();
    if child < 0 {
        return false;
    }
    if child == 0 {
        let delay = TimeSpec {
            sec: 0,
            nsec: 50_000_000,
        };
        let mut delay_rem = TimeSpec::default();
        if nanosleep(&delay, &mut delay_rem) != 0 || kill(parent as usize, signo) != 0 {
            exit_group(92);
        }
        exit_group(0);
    }

    let request = TimeSpec {
        sec: 0,
        nsec: 400_000_000,
    };
    let mut remaining = TimeSpec { sec: 9, nsec: 9 };
    let started = time_get();
    let result = match wait_kind {
        0 => nanosleep(&request, &mut remaining),
        1 => {
            let mut fds: [PollFd; 0] = [];
            ppoll_raw(&mut fds, &request, null(), 0)
        }
        2 => pselect6_raw(0, 0, 0, 0, &request, 0),
        3 => {
            let epfd = epoll_create1(0);
            if epfd < 0 {
                epfd
            } else {
                let mut epoll_pipe = [-1i32; 2];
                let mut event = [0u8; 12];
                event[..4].copy_from_slice(&1u32.to_ne_bytes());
                let result = if pipe(&mut epoll_pipe) != 0
                    || epoll_ctl(epfd as usize, 1, epoll_pipe[0] as usize, event.as_ptr()) != 0
                {
                    -1
                } else {
                    epoll_pwait(epfd as usize, event.as_mut_ptr(), 1, 400, null(), 0)
                };
                if epoll_pipe[0] >= 0 {
                    let _ = close(epoll_pipe[0] as usize);
                }
                if epoll_pipe[1] >= 0 {
                    let _ = close(epoll_pipe[1] as usize);
                }
                let _ = close(epfd as usize);
                result
            }
        }
        4 => clock_nanosleep_raw(1, 0, &request, &mut remaining),
        5 => {
            let mut deadline = TimeSpec::default();
            if clock_gettime_raw(1, &mut deadline) != 0 {
                -1
            } else {
                deadline.nsec += 400_000_000;
                if deadline.nsec >= 1_000_000_000 {
                    deadline.sec += 1;
                    deadline.nsec -= 1_000_000_000;
                }
                clock_nanosleep_raw(1, 1, &deadline, &mut remaining)
            }
        }
        _ => return false,
    };
    let elapsed = time_get().saturating_sub(started);
    let mut status = -1;
    let waited = wait4_raw(child, &mut status, 0, null_mut());
    let count = SIGUSR1_COUNT.load(Ordering::SeqCst);

    let timing_ok = if default_ignored {
        result == 0 && elapsed >= 300
    } else {
        let base = result == -EINTR && elapsed < 300 && count == 1;
        if wait_kind == 0 || wait_kind == 4 {
            let remaining_ms = remaining
                .sec
                .saturating_mul(1000)
                .saturating_add(remaining.nsec / 1_000_000);
            let total = (elapsed.max(0) as usize).saturating_add(remaining_ms);
            base && remaining_ms > 100 && remaining_ms <= 400 && (300..=500).contains(&total)
        } else if wait_kind == 5 {
            base && remaining.sec == 9 && remaining.nsec == 9
        } else {
            base
        }
    };
    if !timing_ok || waited != child || status != 0 {
        println!(
            "SIGNAL_PHASE5_FAIL timeout_nonrestart kind={} restart={} ignored={} result={} elapsed={} rem={}.{} count={} waited={} status={}",
            wait_kind,
            restart,
            default_ignored,
            result,
            elapsed,
            remaining.sec,
            remaining.nsec,
            count,
            waited,
            status
        );
        return false;
    }
    true
}

fn test_timeout_nonrestart() -> bool {
    for wait_kind in 0..6 {
        if !run_timeout_signal_case(wait_kind, false, false)
            || !run_timeout_signal_case(wait_kind, true, false)
            || !run_timeout_signal_case(wait_kind, false, true)
        {
            return false;
        }
    }
    let _ = sigaction_raw(SIGUSR1, &SignalAction::default(), null_mut(), SIGSET_SIZE);
    println!("SIGNAL_PHASE5 timeout_nonrestart PASS");
    true
}

fn exec_pending_target() -> i32 {
    let mut mask = 0u64;
    let mut pending = 0u64;
    let query = sigprocmask_raw(usize::MAX, null(), &mut mask, SIGSET_SIZE);
    let pending_result = sigpending_raw(&mut pending, SIGSET_SIZE);
    let bit = signal_bit(SIGUSR1);
    if query != 0 || pending_result != 0 || mask & bit == 0 || pending & bit == 0 {
        println!(
            "SIGNAL_PHASE5_FAIL exec_pending query={} pending_result={} mask={:#x} pending={:#x}",
            query, pending_result, mask, pending
        );
        return 1;
    }
    let mut usage = RUsage::default();
    if getrusage_raw(0, &mut usage) != 0 || usage.ru_nsignals != 0 {
        println!(
            "SIGNAL_PHASE5_FAIL rusage_nsignals value={}",
            usage.ru_nsignals
        );
        return 1;
    }
    println!("SIGNAL_PHASE5 rusage_nsignals_linux_zero PASS");
    println!("SIGNAL_PHASE5 exec_pending PASS");
    println!("SIGNAL_PHASE5 ALL PASS");
    0
}

fn test_pending_survives_exec() -> i32 {
    let block = signal_bit(SIGUSR1);
    if sigprocmask_raw(SIG_BLOCK, &block, null_mut(), SIGSET_SIZE) != 0 {
        println!("SIGNAL_PHASE5_FAIL block");
        return 1;
    }
    let pid = getpid();
    if pid <= 0 || kill(pid as usize, SIGUSR1) != 0 {
        println!("SIGNAL_PHASE5_FAIL queue pid={}", pid);
        return 1;
    }
    let mut pending = 0u64;
    if sigpending_raw(&mut pending, SIGSET_SIZE) != 0 || pending & block == 0 {
        println!("SIGNAL_PHASE5_FAIL pre_exec_pending={:#x}", pending);
        return 1;
    }

    let argv = [
        "signal_phase5_probe\0".as_ptr(),
        "--exec-target\0".as_ptr(),
        core::ptr::null(),
    ];
    let result = exec("signal_phase5_probe\0", &argv);
    println!("SIGNAL_PHASE5_FAIL exec result={}", result);
    1
}

#[unsafe(no_mangle)]
fn main(argc: usize, argv: &[&str]) -> i32 {
    if argc == 2 && argv[1] == "--exec-target" {
        return exec_pending_target();
    }
    if !test_query_ignores_how()
        || !test_sigaction_validation()
        || !test_sigqueueinfo_null_signal()
        || !test_pending_queue_semantics()
        || !test_pipe_read_restart()
        || !test_pipe_write_restart()
        || !test_pipe_vectored_restart()
        || !test_futex_restart()
        || !test_timeout_nonrestart()
        || !test_sigchld_autoreap()
        || !test_sigchld_nocldstop()
    {
        return 1;
    }
    test_pending_survives_exec()
}
