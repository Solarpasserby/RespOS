#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null;
use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{
    TimeSpec, fork, futex_raw, shmat, shmctl, shmdt, shmget, time_get, waitpid, yield_,
};

const PAGE_SIZE: usize = 4096;
const IPC_PRIVATE: isize = 0;
const IPC_CREAT: usize = 0o1000;
const IPC_RMID: usize = 0;
const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;

fn word(base: usize, index: usize) -> &'static AtomicU32 {
    unsafe { &*((base + index * core::mem::size_of::<u32>()) as *const AtomicU32) }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let shmid = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    assert!(shmid > 0, "shmget failed: {}", shmid);

    let parent_addr = shmat(shmid as usize, 0, 0);
    assert!(parent_addr > 0, "parent shmat failed: {}", parent_addr);
    let parent_addr = parent_addr as usize;
    word(parent_addr, 0).store(0, Ordering::Release);
    word(parent_addr, 1).store(0, Ordering::Release);
    word(parent_addr, 2).store(0x5a17_c0de, Ordering::Release);

    let child = fork();
    assert!(child >= 0, "fork failed: {}", child);
    if child == 0 {
        let child_addr = shmat(shmid as usize, 0, 0);
        assert!(child_addr > 0, "child shmat failed: {}", child_addr);
        let child_addr = child_addr as usize;
        assert_ne!(child_addr, parent_addr);
        assert_eq!(word(child_addr, 2).load(Ordering::Acquire), 0x5a17_c0de);

        word(child_addr, 1).store(1, Ordering::Release);
        let timeout = TimeSpec { sec: 2, nsec: 0 };
        let result = futex_raw(
            child_addr as *const u32,
            FUTEX_WAIT,
            0,
            &timeout as *const TimeSpec,
        );
        if result != 0 {
            println!("SYSV_SHM_FUTEX child wait failed: {}", result);
            return 1;
        }
        assert_eq!(shmdt(child_addr), 0);
        return 0;
    }

    while word(parent_addr, 1).load(Ordering::Acquire) != 1 {
        let _ = yield_();
    }

    let deadline = time_get().saturating_add(1500);
    let mut wake_result = 0;
    while time_get() < deadline {
        wake_result = futex_raw(parent_addr as *const u32, FUTEX_WAKE, 1, null());
        if wake_result == 1 {
            break;
        }
        let _ = yield_();
    }

    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(shmdt(parent_addr), 0);
    assert_eq!(shmctl(shmid as usize, IPC_RMID, 0), 0);

    if wake_result != 1 || status != 0 {
        println!(
            "SYSV_SHM_FUTEX_EXPECTED_FAIL wake={} child_status={}",
            wake_result, status
        );
        return 1;
    }

    println!("SYSV_SHM_FUTEX PASS");
    0
}
