#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null_mut;
use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{
    SIGUSR1, SignalAction, exec, exit_group, fork, getpid, gettid, kill, mmap_raw, sigaction,
    time_get, wait4_raw, yield_,
};

const PAGE_SIZE: usize = 4096;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;
const CLONE_THREAD_FLAGS: usize = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 16);
const THREAD_STACK_SIZE: usize = 32 * 1024;
const LEADER_EXIT_STATUS: i32 = 42;
const WORKER_EXIT_STATUS: i32 = 7;
const EXEC_FAILURE_STATUS: i32 = 111;
const EXEC_TARGET_STATUS: i32 = 23;

#[repr(C)]
struct LeaderExitState {
    worker_ready: AtomicU32,
    leader_may_exit: AtomicU32,
    worker_survived: AtomicU32,
}

#[repr(C)]
struct LeaderIdentityState {
    worker_ready: AtomicU32,
    leader_may_exit: AtomicU32,
    worker_survived: AtomicU32,
    signal_seen: AtomicU32,
    ids_valid: AtomicU32,
    process_pid: AtomicU32,
}

#[repr(align(16))]
struct ThreadStack([u8; THREAD_STACK_SIZE]);

static mut THREAD_STACK: ThreadStack = ThreadStack([0; THREAD_STACK_SIZE]);
static IDENTITY_SIGNAL_SEEN: AtomicU32 = AtomicU32::new(0);

fn identity_signal_handler() {
    IDENTITY_SIGNAL_SEEN.store(1, Ordering::Release);
}

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global task_phase5_clone_thread
    .type task_phase5_clone_thread, @function
task_phase5_clone_thread:
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
    .global task_phase5_clone_thread
    .type task_phase5_clone_thread, @function
task_phase5_clone_thread:
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
    fn task_phase5_clone_thread(
        flags: usize,
        stack_top: usize,
        entry: extern "C" fn(usize) -> i32,
        arg: usize,
    ) -> isize;
}

fn clone_thread(entry: extern "C" fn(usize) -> i32, arg: usize) -> isize {
    let stack_top =
        unsafe { core::ptr::addr_of_mut!(THREAD_STACK.0) as *mut u8 as usize + THREAD_STACK_SIZE };
    unsafe { task_phase5_clone_thread(CLONE_THREAD_FLAGS, stack_top, entry, arg) }
}

fn wait_for_process(pid: isize) -> Option<i32> {
    let mut status = 0;
    loop {
        let result = wait4_raw(pid, &mut status, 0, null_mut());
        if result == pid {
            return Some(status);
        }
        if result != -4 {
            println!("TASK_PHASE5 wait4 pid={} failed: {}", pid, result);
            return None;
        }
    }
}

extern "C" fn leader_exit_worker(arg: usize) -> i32 {
    let state = unsafe { &*(arg as *const LeaderExitState) };
    state.worker_ready.store(1, Ordering::Release);
    while state.leader_may_exit.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }

    let deadline = time_get().saturating_add(100);
    while time_get() < deadline {
        let _ = yield_();
    }
    state.worker_survived.store(1, Ordering::Release);
    let _ = exit_group(WORKER_EXIT_STATUS);
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn leader_exit_worker_raw(arg: usize) -> i32 {
    let state = unsafe { &*(arg as *const LeaderExitState) };
    state.worker_ready.store(1, Ordering::Release);
    while state.leader_may_exit.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }

    let deadline = time_get().saturating_add(100);
    while time_get() < deadline {
        let _ = yield_();
    }
    state.worker_survived.store(1, Ordering::Release);
    WORKER_EXIT_STATUS
}

fn test_leader_sys_exit_case(
    worker: extern "C" fn(usize) -> i32,
    expected_status: i32,
    label: &str,
) -> bool {
    let mapping = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    if mapping <= 0 {
        println!("TASK_PHASE5 leader_sys_exit mmap failed: {}", mapping);
        return false;
    }
    let state = unsafe { &*(mapping as *const LeaderExitState) };
    state.worker_ready.store(0, Ordering::Relaxed);
    state.leader_may_exit.store(0, Ordering::Relaxed);
    state.worker_survived.store(0, Ordering::Relaxed);

    let child = fork();
    if child < 0 {
        println!("TASK_PHASE5 leader_sys_exit fork failed: {}", child);
        return false;
    }
    if child == 0 {
        if clone_thread(worker, mapping as usize) <= 0 {
            let _ = exit_group(121);
        }
        while state.worker_ready.load(Ordering::Acquire) == 0 {
            let _ = yield_();
        }
        state.leader_may_exit.store(1, Ordering::Release);
        user_lib::exit(LEADER_EXIT_STATUS);
        loop {
            core::hint::spin_loop();
        }
    }

    let Some(status) = wait_for_process(child) else {
        return false;
    };
    let survived = state.worker_survived.load(Ordering::Acquire);
    if status != expected_status << 8 || survived != 1 {
        println!(
            "TASK_PHASE5_EXPECTED_FAIL {} status={} survived={} expected_status={}",
            label,
            status,
            survived,
            expected_status << 8
        );
        return false;
    }
    println!("TASK_PHASE5 {} PASS", label);
    true
}

extern "C" fn leader_identity_worker(arg: usize) -> i32 {
    let state = unsafe { &*(arg as *const LeaderIdentityState) };
    state.worker_ready.store(1, Ordering::Release);
    while state.leader_may_exit.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }

    for _ in 0..1000 {
        let _ = yield_();
    }
    let process_pid = state.process_pid.load(Ordering::Acquire) as isize;
    state.ids_valid.store(
        u32::from(getpid() == process_pid && getpid() != gettid()),
        Ordering::Release,
    );
    state.worker_survived.store(1, Ordering::Release);
    while IDENTITY_SIGNAL_SEEN.load(Ordering::Acquire) == 0 {
        let _ = yield_();
    }
    state.signal_seen.store(1, Ordering::Release);
    WORKER_EXIT_STATUS
}

fn test_leader_exit_keeps_process_identity() -> bool {
    const WNOHANG: usize = 1;
    let mapping = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    if mapping <= 0 {
        println!("TASK_PHASE5 leader_identity mmap failed: {}", mapping);
        return false;
    }
    let state = unsafe { &*(mapping as *const LeaderIdentityState) };
    state.worker_ready.store(0, Ordering::Relaxed);
    state.leader_may_exit.store(0, Ordering::Relaxed);
    state.worker_survived.store(0, Ordering::Relaxed);
    state.signal_seen.store(0, Ordering::Relaxed);
    state.ids_valid.store(0, Ordering::Relaxed);
    state.process_pid.store(0, Ordering::Relaxed);
    IDENTITY_SIGNAL_SEEN.store(0, Ordering::Relaxed);

    let child = fork();
    if child < 0 {
        println!("TASK_PHASE5 leader_identity fork failed: {}", child);
        return false;
    }
    if child == 0 {
        let action = SignalAction {
            handler: identity_signal_handler as usize,
            ..SignalAction::default()
        };
        if sigaction(SIGUSR1, Some(&action), None) != 0 {
            let _ = exit_group(124);
        }
        state.process_pid.store(getpid() as u32, Ordering::Release);
        if clone_thread(leader_identity_worker, mapping as usize) <= 0 {
            let _ = exit_group(125);
        }
        while state.worker_ready.load(Ordering::Acquire) == 0 {
            let _ = yield_();
        }
        state.leader_may_exit.store(1, Ordering::Release);
        user_lib::exit(LEADER_EXIT_STATUS);
        loop {
            core::hint::spin_loop();
        }
    }

    let mut status = 0;
    for _ in 0..100_000 {
        if state.worker_survived.load(Ordering::Acquire) != 0 {
            break;
        }
        let result = wait4_raw(child, &mut status, WNOHANG, null_mut());
        if result == child {
            println!(
                "TASK_PHASE5_EXPECTED_FAIL leader_exit_keeps_process_identity early_status={} survived=0",
                status
            );
            return false;
        }
        if result != 0 && result != -4 {
            println!(
                "TASK_PHASE5 leader_identity WNOHANG failed: result={} status={}",
                result, status
            );
            return false;
        }
        let _ = yield_();
    }
    if state.worker_survived.load(Ordering::Acquire) == 0 {
        println!("TASK_PHASE5_EXPECTED_FAIL leader_exit_keeps_process_identity worker_timeout");
        return false;
    }
    let nohang = wait4_raw(child, &mut status, WNOHANG, null_mut());
    let signal_result = kill(child as usize, SIGUSR1);
    let final_status = if nohang == 0 && signal_result == 0 {
        wait_for_process(child)
    } else {
        None
    };
    let ok = nohang == 0
        && signal_result == 0
        && final_status == Some(WORKER_EXIT_STATUS << 8)
        && state.ids_valid.load(Ordering::Acquire) == 1
        && state.signal_seen.load(Ordering::Acquire) == 1;
    if !ok {
        println!(
            "TASK_PHASE5_EXPECTED_FAIL leader_exit_keeps_process_identity nohang={} kill={} final={:?} ids={} signal={}",
            nohang,
            signal_result,
            final_status,
            state.ids_valid.load(Ordering::Acquire),
            state.signal_seen.load(Ordering::Acquire)
        );
        return false;
    }
    println!("TASK_PHASE5 leader_exit_keeps_process_identity PASS");
    true
}

extern "C" fn nonleader_exec_worker(_arg: usize) -> i32 {
    let argv = ["task_phase5_exec_target\0".as_ptr(), core::ptr::null()];
    let result = exec("task_phase5_exec_target\0", &argv);
    println!("TASK_PHASE5 nonleader exec failed: {}", result);
    let _ = exit_group(EXEC_FAILURE_STATUS);
    loop {
        core::hint::spin_loop();
    }
}

fn test_nonleader_exec() -> bool {
    let child = fork();
    if child < 0 {
        println!("TASK_PHASE5 nonleader_exec fork failed: {}", child);
        return false;
    }
    if child == 0 {
        if clone_thread(nonleader_exec_worker, 0) <= 0 {
            let _ = exit_group(123);
        }
        loop {
            let _ = yield_();
        }
    }

    let Some(status) = wait_for_process(child) else {
        return false;
    };
    if status != EXEC_TARGET_STATUS << 8 {
        println!(
            "TASK_PHASE5_EXPECTED_FAIL nonleader_exec status={} expected_status={}",
            status,
            EXEC_TARGET_STATUS << 8
        );
        return false;
    }
    println!("TASK_PHASE5 nonleader_exec PASS");
    true
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let leader_exit_group_ok = test_leader_sys_exit_case(
        leader_exit_worker,
        WORKER_EXIT_STATUS,
        "leader_exit_then_exit_group",
    );
    let leader_exit_worker_ok = test_leader_sys_exit_case(
        leader_exit_worker_raw,
        WORKER_EXIT_STATUS,
        "leader_exit_then_worker_exit",
    );
    let leader_identity_ok = test_leader_exit_keeps_process_identity();
    let nonleader_exec_ok = test_nonleader_exec();
    if leader_exit_group_ok && leader_exit_worker_ok && leader_identity_ok && nonleader_exec_ok {
        println!("TASK_PHASE5 ALL PASS");
        0
    } else {
        println!("TASK_PHASE5 CURRENT DIFFERENCES CONFIRMED");
        1
    }
}
