#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{TimeSpec, fork, futex_raw, kill, mmap_raw, time_get, wait4_raw, yield_};

const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const SIGKILL: i32 = 9;
const EINTR: isize = 4;
const PAGE_SIZE: usize = 4096;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;
const CLONE_THREAD_FLAGS: usize = (1 << 8) | (1 << 11) | (1 << 16);
const THREAD_STACK_SIZE: usize = 16 * 1024;

#[repr(align(16))]
struct ThreadStack([u8; THREAD_STACK_SIZE]);

static mut THREAD_STACK: ThreadStack = ThreadStack([0; THREAD_STACK_SIZE]);

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global task_a_clone_thread
    .type task_a_clone_thread, @function
task_a_clone_thread:
    addi a1, a1, -16
    sd a2, 0(a1)
    sd a3, 8(a1)
    li a2, 0
    li a3, 0
    li a4, 0
    li a7, 220
    ecall
    bnez a0, 1f
    ld t0, 0(sp)
    ld a0, 8(sp)
    jalr t0
    li a7, 93
    ecall
1:
    ret
"#
);

#[cfg(target_arch = "loongarch64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global task_a_clone_thread
    .type task_a_clone_thread, @function
task_a_clone_thread:
    addi.d $a1, $a1, -16
    st.d $a2, $a1, 0
    st.d $a3, $a1, 8
    ori $a2, $zero, 0
    ori $a3, $zero, 0
    ori $a4, $zero, 0
    addi.w $a7, $zero, 220
    syscall 0
    bnez $a0, 1f
    ld.d $t0, $sp, 0
    ld.d $a0, $sp, 8
    jirl $ra, $t0, 0
    addi.w $a7, $zero, 93
    syscall 0
1:
    jirl $zero, $ra, 0
"#
);

unsafe extern "C" {
    fn task_a_clone_thread(
        flags: usize,
        stack_top: usize,
        entry: extern "C" fn(usize) -> i32,
        arg: usize,
    ) -> isize;
}

fn delay_ms(ms: isize) {
    let deadline = time_get().saturating_add(ms);
    while time_get() < deadline {
        let _ = yield_();
    }
}

fn shared_atomic(shared: usize, offset: usize) -> &'static AtomicU32 {
    unsafe { &*((shared + offset) as *const AtomicU32) }
}

extern "C" fn waiter_thread(shared: usize) -> i32 {
    let futex_ptr = shared as *const u32;
    let state = shared_atomic(shared, 4);
    state.store(1, Ordering::Release);
    let timeout = TimeSpec { sec: 10, nsec: 0 };
    let result = futex_raw(futex_ptr, FUTEX_WAIT, 0, &timeout);
    state.store(0xff00 | ((-result) as u32 & 0xff), Ordering::Release);
    91
}

fn run_owner(shared: usize) -> ! {
    let stack_top =
        unsafe { core::ptr::addr_of_mut!(THREAD_STACK.0) as *mut u8 as usize + THREAD_STACK_SIZE };
    let tid = unsafe { task_a_clone_thread(CLONE_THREAD_FLAGS, stack_top, waiter_thread, shared) };
    assert!(tid > 0);

    let state = shared_atomic(shared, 4);
    while state.load(Ordering::Acquire) != 1 {
        let _ = yield_();
    }
    delay_ms(100);
    state.store(2, Ordering::Release);
    loop {
        let _ = yield_();
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let shared = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    assert!(shared > 0);
    let futex_ptr = shared as *mut u32;
    unsafe {
        futex_ptr.write(0);
    }
    shared_atomic(shared as usize, 4).store(0, Ordering::Release);

    let owner = fork();
    assert!(owner >= 0);
    if owner == 0 {
        run_owner(shared as usize);
    }

    let state = shared_atomic(shared as usize, 4);
    while state.load(Ordering::Acquire) != 2 {
        let _ = yield_();
    }
    delay_ms(100);
    assert_eq!(kill(owner as usize, SIGKILL), 0);

    let mut status = 0;
    loop {
        let ret = wait4_raw(owner, &mut status, 0, null_mut());
        if ret == owner {
            break;
        }
        assert_eq!(ret, -EINTR);
    }
    assert_eq!(status, SIGKILL);
    assert_eq!(futex_raw(futex_ptr, FUTEX_WAKE, 1, null()), 0);
    assert_eq!(state.load(Ordering::Acquire), 2);
    println!("[task-a-futex-exit] PASS owner={} status={}", owner, status);
    0
}
