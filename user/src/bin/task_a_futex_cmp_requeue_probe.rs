#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{
    TimeSpec, exit, fork, futex_cmp_requeue_raw, futex_raw, mmap_raw, time_get, wait4_raw, yield_,
};

const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const EAGAIN: isize = 11;
const EINTR: isize = 4;
const PAGE_SIZE: usize = 4096;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;

fn shared_atomic(shared: usize, offset: usize) -> &'static AtomicU32 {
    unsafe { &*((shared + offset) as *const AtomicU32) }
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
        assert_eq!(ret, -EINTR);
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let shared = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    assert!(shared > 0);
    let shared = shared as usize;
    let source = shared_atomic(shared, 0);
    let target = shared_atomic(shared, 4);
    let state = shared_atomic(shared, 8);
    source.store(0, Ordering::Release);
    target.store(0, Ordering::Release);
    state.store(0, Ordering::Release);

    let source_ptr = source as *const AtomicU32 as *const u32;
    let target_ptr = target as *const AtomicU32 as *const u32;

    let waiter = fork();
    assert!(waiter >= 0);
    if waiter == 0 {
        let timeout = TimeSpec { sec: 2, nsec: 0 };
        let result = futex_raw(source_ptr, FUTEX_WAIT, 0, &timeout);
        assert_eq!(result, 0);
        exit(0);
        unreachable!();
    }

    delay_ms(100);
    let changer = fork();
    assert!(changer >= 0);
    if changer == 0 {
        while state.load(Ordering::Acquire) != 1 {
            let _ = yield_();
        }
        source.store(1, Ordering::Release);
        state.store(2, Ordering::Release);
        exit(0);
        unreachable!();
    }

    state.store(1, Ordering::Release);
    let result = futex_cmp_requeue_raw(source_ptr, 0, 1, target_ptr, 0);
    assert_eq!(result, -EAGAIN);
    assert_eq!(state.load(Ordering::Acquire), 2);
    assert_eq!(reap(changer), 0);

    // Failed comparison must leave the waiter on the source queue.
    assert_eq!(futex_raw(target_ptr, FUTEX_WAKE, 1, null()), 0);
    assert_eq!(futex_raw(source_ptr, FUTEX_WAKE, 1, null()), 1);
    assert_eq!(reap(waiter), 0);
    assert_eq!(futex_raw(source_ptr, FUTEX_WAKE, 1, null()), 0);
    assert_eq!(futex_raw(target_ptr, FUTEX_WAKE, 1, null()), 0);
    println!("[task-a-futex-cmp-requeue] PASS");
    0
}
