#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{fork, shmat, shmctl, shmdt, shmget, time_get, waitpid, yield_};

const PAGE_SIZE: usize = 4096;
const IPC_PRIVATE: isize = 0;
const IPC_CREAT: usize = 0o1000;
const IPC_RMID: usize = 0;
const IPC_STAT: usize = 2;
const CLONE_THREAD_FLAGS: usize = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 16);
const THREAD_STACK_SIZE: usize = 16 * 1024;

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct IpcPerm {
    key: i32,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    mode: u32,
    seq: i32,
    pad1: isize,
    pad2: isize,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ShmidDs {
    shm_perm: IpcPerm,
    shm_segsz: usize,
    shm_atime: isize,
    shm_dtime: isize,
    shm_ctime: isize,
    shm_cpid: i32,
    shm_lpid: i32,
    shm_nattch: usize,
    pad1: usize,
    pad2: usize,
}

#[repr(C)]
struct ProbePage {
    child_ready: AtomicU32,
    child_release: AtomicU32,
}

#[repr(align(16))]
struct ThreadStack([u8; THREAD_STACK_SIZE]);

static mut THREAD_STACK: ThreadStack = ThreadStack([0; THREAD_STACK_SIZE]);
static THREAD_READY: AtomicU32 = AtomicU32::new(0);
static THREAD_RELEASE: AtomicU32 = AtomicU32::new(0);
static THREAD_DONE: AtomicU32 = AtomicU32::new(0);

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global sysv_shm_nattch_clone_thread
    .type sysv_shm_nattch_clone_thread, @function
sysv_shm_nattch_clone_thread:
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
    .global sysv_shm_nattch_clone_thread
    .type sysv_shm_nattch_clone_thread, @function
sysv_shm_nattch_clone_thread:
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
    fn sysv_shm_nattch_clone_thread(
        flags: usize,
        stack_top: usize,
        entry: extern "C" fn(usize) -> i32,
        arg: usize,
    ) -> isize;
}

extern "C" fn thread_worker(_arg: usize) -> i32 {
    THREAD_READY.store(1, Ordering::Release);
    while THREAD_RELEASE.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }
    THREAD_DONE.store(1, Ordering::Release);
    0
}

fn nattch(shmid: usize) -> usize {
    let mut ds = ShmidDs::default();
    let ds_ptr = &mut ds as *mut ShmidDs as usize;
    assert_eq!(shmctl(shmid, IPC_STAT, ds_ptr), 0);
    ds.shm_nattch
}

fn wait_for_nattch(shmid: usize, expected: usize) {
    let deadline = time_get().saturating_add(1000);
    while time_get() < deadline {
        if nattch(shmid) == expected {
            return;
        }
        let _ = yield_();
    }
    assert_eq!(nattch(shmid), expected);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let shmid = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    assert!(shmid > 0, "shmget failed: {}", shmid);
    let shmid = shmid as usize;
    assert_eq!(nattch(shmid), 0);

    let first = shmat(shmid, 0, 0);
    assert!(first > 0, "first shmat failed: {}", first);
    let first = first as usize;
    assert_eq!(nattch(shmid), 1);
    let second = shmat(shmid, 0, 0);
    assert!(second > 0, "second shmat failed: {}", second);
    let second = second as usize;
    assert_eq!(nattch(shmid), 2);

    THREAD_READY.store(0, Ordering::Relaxed);
    THREAD_RELEASE.store(0, Ordering::Relaxed);
    THREAD_DONE.store(0, Ordering::Relaxed);
    let stack_top =
        unsafe { core::ptr::addr_of_mut!(THREAD_STACK.0) as *mut u8 as usize + THREAD_STACK_SIZE };
    let tid =
        unsafe { sysv_shm_nattch_clone_thread(CLONE_THREAD_FLAGS, stack_top, thread_worker, 0) };
    assert!(tid > 0, "clone thread failed: {}", tid);
    while THREAD_READY.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }
    let thread_count = nattch(shmid);
    THREAD_RELEASE.store(1, Ordering::Release);
    while THREAD_DONE.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }
    wait_for_nattch(shmid, 2);

    let page = unsafe { &*(first as *const ProbePage) };
    page.child_ready.store(0, Ordering::Relaxed);
    page.child_release.store(0, Ordering::Relaxed);
    let child = fork();
    assert!(child >= 0, "fork failed: {}", child);
    if child == 0 {
        page.child_ready.store(1, Ordering::Release);
        while page.child_release.load(Ordering::Acquire) == 0 {
            let _ = yield_();
        }
        return 0;
    }

    while page.child_ready.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }
    assert_eq!(nattch(shmid), 4);
    page.child_release.store(1, Ordering::Release);
    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(nattch(shmid), 2);

    assert_eq!(shmdt(second), 0);
    assert_eq!(nattch(shmid), 1);
    assert_eq!(shmdt(first), 0);
    assert_eq!(nattch(shmid), 0);
    assert_eq!(shmctl(shmid, IPC_RMID, 0), 0);

    if thread_count != 2 {
        println!(
            "SYSV_SHM_NATTCH_EXPECTED_FAIL thread_count={}",
            thread_count
        );
        return 1;
    }

    println!("SYSV_SHM_NATTCH PASS");
    0
}
