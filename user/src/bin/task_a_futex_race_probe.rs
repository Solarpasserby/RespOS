#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    SIGUSR1, SignalAction, TimeSpec, exit, fork, futex_raw, getpid, kill, mmap_raw, sigaction,
    time_get, wait4_raw, yield_,
};

const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const EINTR: isize = 4;
const ETIMEDOUT: isize = 110;
const PAGE_SIZE: usize = 4096;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;
static SIGNAL_COUNT: AtomicUsize = AtomicUsize::new(0);

fn signal_handler() {
    SIGNAL_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn delay_ms(ms: isize) {
    let deadline = time_get().saturating_add(ms);
    while time_get() < deadline {
        let _ = yield_();
    }
}

fn reap(pid: isize) -> i32 {
    let mut status = 0;
    loop {
        let ret = wait4_raw(pid, &mut status, 0, null_mut());
        if ret == pid {
            return status;
        }
        assert!(ret == -EINTR || ret == -11);
        let _ = yield_();
    }
}

fn run_case(
    futex_ptr: *const u32,
    name: &str,
    wake_ms: isize,
    signal_ms: isize,
    timeout_ms: usize,
    expected: isize,
) {
    println!("[task-a-futex-race] {} start", name);
    let parent = getpid();
    let before_signals = SIGNAL_COUNT.load(Ordering::SeqCst);

    let waker = fork();
    assert!(waker >= 0);
    if waker == 0 {
        delay_ms(wake_ms);
        let woke = futex_raw(futex_ptr, FUTEX_WAKE, 1, null());
        println!("[task-a-futex-race] {} waker woke={}", name, woke);
        assert!(woke == 0 || woke == 1);
        exit(woke as i32);
        unreachable!();
    }

    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(signal_ms);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        exit(0);
        unreachable!();
    }

    let timeout = TimeSpec {
        sec: timeout_ms / 1000,
        nsec: (timeout_ms % 1000) * 1_000_000,
    };
    let result = futex_raw(futex_ptr, FUTEX_WAIT, 0, &timeout);
    println!("[task-a-futex-race] {} wait result={}", name, result);
    assert_eq!(result, expected);

    let wake_status = reap(waker);
    println!("[task-a-futex-race] {} waker status={}", name, wake_status);
    assert_eq!(reap(signaler), 0);
    println!("[task-a-futex-race] {} signaler reaped", name);
    assert_eq!(SIGNAL_COUNT.load(Ordering::SeqCst), before_signals + 1);
    let expected_wake_count = if expected == 0 { 1 } else { 0 };
    assert_eq!(wake_status, expected_wake_count << 8);
    assert_eq!(futex_raw(futex_ptr, FUTEX_WAKE, 1, null()), 0);
    println!("[task-a-futex-race] {} PASS", name);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[task-a-futex-race] install handler");
    let action = SignalAction {
        handler: signal_handler as usize,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&action), None), 0);

    let shared_page = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    assert!(shared_page > 0);
    let futex_ptr = shared_page as *mut u32;
    unsafe {
        futex_ptr.write(0);
    }

    run_case(futex_ptr, "wake-first", 100, 250, 400, 0);
    run_case(futex_ptr, "signal-first", 250, 100, 400, -EINTR);
    run_case(futex_ptr, "timeout-first", 250, 350, 100, -ETIMEDOUT);
    println!("[task-a-futex-race] ALL PASS");
    0
}
