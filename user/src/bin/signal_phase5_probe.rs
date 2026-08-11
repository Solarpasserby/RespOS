#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null, null_mut};
use user_lib::{
    SignalAction, exec, getpid, kill, sigaction_raw, sigpending_raw, sigprocmask_raw,
    sigqueueinfo_raw,
};

const SIG_BLOCK: usize = 0;
const SIGUSR1: i32 = 10;
const SIGSET_SIZE: usize = core::mem::size_of::<u64>();
const SI_QUEUE: i32 = -1;
const EFAULT: isize = 14;
const EINVAL: isize = 22;

#[repr(C)]
struct LinuxSigInfo {
    signo: i32,
    errno: i32,
    code: i32,
    pad: [i32; 29],
}

impl LinuxSigInfo {
    fn queued(signo: i32, pid: i32) -> Self {
        let mut info = Self {
            signo,
            errno: 0,
            code: SI_QUEUE,
            pad: [0; 29],
        };
        info.pad[0] = pid;
        info
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
    let info = LinuxSigInfo::queued(0, pid as i32);
    let result = sigqueueinfo_raw(pid as usize, 0, &info as *const LinuxSigInfo as *const u8);
    if result != 0 {
        println!("SIGNAL_PHASE5_FAIL sigqueueinfo_zero result={}", result);
        return false;
    }
    println!("SIGNAL_PHASE5 sigqueueinfo_zero PASS");
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
    if !test_query_ignores_how() || !test_sigaction_validation() || !test_sigqueueinfo_null_signal()
    {
        return 1;
    }
    test_pending_survives_exec()
}
