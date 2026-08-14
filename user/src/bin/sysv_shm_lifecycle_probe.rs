#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{SIGKILL, exec, exit, fork, getpid, kill, shmat, shmctl, shmdt, shmget, waitpid};

const PAGE_SIZE: usize = 4096;
const IPC_CREAT: usize = 0o1000;
const IPC_EXCL: usize = 0o2000;
const IPC_RMID: usize = 0;
const ENOENT: isize = 2;
const EINVAL: isize = 22;

fn probe_key(slot: usize) -> isize {
    (0x5200_0000usize | (((getpid() as usize) & 0xffff) << 4) | (slot & 0xf)) as isize
}

fn create_segment(key: isize) -> isize {
    let shmid = shmget(key, PAGE_SIZE, IPC_CREAT | IPC_EXCL | 0o600);
    assert!(shmid > 0, "shmget({:#x}) failed: {}", key, shmid);
    shmid
}

fn read_word(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn write_word(addr: usize, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

fn stale_id_is_gone(shmid: usize) -> bool {
    let result = shmat(shmid, 0, 0);
    if result == -EINVAL {
        return true;
    }
    assert!(
        result > 0,
        "stale shmat returned unexpected errno: {}",
        result
    );
    assert_eq!(shmdt(result as usize), 0);
    false
}

fn test_explicit_last_detach() {
    let key = probe_key(1);
    let old_id = create_segment(key);
    let first = shmat(old_id as usize, 0, 0);
    assert!(first > 0, "first shmat failed: {}", first);
    let first = first as usize;
    write_word(first, 0x51a7_c0de);

    assert_eq!(shmctl(old_id as usize, IPC_RMID, 0), 0);
    assert_eq!(shmget(key, PAGE_SIZE, 0), -ENOENT);

    let replacement = create_segment(key);
    assert_ne!(replacement, old_id);

    let second = shmat(old_id as usize, 0, 0);
    assert!(second > 0, "post-RMID shmat failed: {}", second);
    let second = second as usize;
    assert_eq!(read_word(second), 0x51a7_c0de);
    assert_eq!(shmdt(second), 0);
    assert_eq!(read_word(first), 0x51a7_c0de);
    assert_eq!(shmdt(first), 0);
    assert!(stale_id_is_gone(old_id as usize));

    assert_eq!(shmctl(replacement as usize, IPC_RMID, 0), 0);
    println!("SYSV_SHM_LIFECYCLE explicit_detach PASS");
}

fn test_exit_without_shmdt() -> bool {
    let key = probe_key(2);
    let shmid = create_segment(key);
    let child = fork();
    assert!(child >= 0, "fork failed: {}", child);
    if child == 0 {
        let mapping = shmat(shmid as usize, 0, 0);
        assert!(mapping > 0, "child shmat failed: {}", mapping);
        write_word(mapping as usize, 0xe817_c0de);
        assert_eq!(shmctl(shmid as usize, IPC_RMID, 0), 0);
        exit(0);
    }

    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    let gone = stale_id_is_gone(shmid as usize);

    let replacement = create_segment(key);
    assert_eq!(shmctl(replacement as usize, IPC_RMID, 0), 0);
    if gone {
        println!("SYSV_SHM_LIFECYCLE exit_cleanup PASS");
    }
    gone
}

fn test_exec_without_shmdt() -> bool {
    let key = probe_key(3);
    let shmid = create_segment(key);
    let child = fork();
    assert!(child >= 0, "fork failed: {}", child);
    if child == 0 {
        let mapping = shmat(shmid as usize, 0, 0);
        assert!(mapping > 0, "child shmat failed: {}", mapping);
        write_word(mapping as usize, 0xec7e_c0de);
        assert_eq!(shmctl(shmid as usize, IPC_RMID, 0), 0);
        let argv = ["true\0".as_ptr(), core::ptr::null()];
        let result = exec("true\0", &argv);
        panic!("exec true failed: {}", result);
    }

    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    let gone = stale_id_is_gone(shmid as usize);

    let replacement = create_segment(key);
    assert_eq!(shmctl(replacement as usize, IPC_RMID, 0), 0);
    if gone {
        println!("SYSV_SHM_LIFECYCLE exec_cleanup PASS");
    }
    gone
}

fn test_fork_inherited_attachment() {
    let key = probe_key(4);
    let shmid = create_segment(key);
    let parent = shmat(shmid as usize, 0, 0);
    assert!(parent > 0, "parent shmat failed: {}", parent);
    let parent = parent as usize;
    write_word(parent, 0xf04c_c0de);
    assert_eq!(shmctl(shmid as usize, IPC_RMID, 0), 0);

    let child = fork();
    assert!(child >= 0, "fork failed: {}", child);
    if child == 0 {
        assert_eq!(read_word(parent), 0xf04c_c0de);
        exit(0);
    }

    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(read_word(parent), 0xf04c_c0de);

    let second = shmat(shmid as usize, 0, 0);
    assert!(second > 0, "inherited segment was reaped early: {}", second);
    let second = second as usize;
    assert_eq!(read_word(second), 0xf04c_c0de);
    assert_eq!(shmdt(second), 0);
    assert_eq!(shmdt(parent), 0);
    assert!(stale_id_is_gone(shmid as usize));
    println!("SYSV_SHM_LIFECYCLE inherited_attach PASS");
}

fn test_signal_exit_without_shmdt() -> bool {
    let key = probe_key(5);
    let shmid = create_segment(key);
    let child = fork();
    assert!(child >= 0, "fork failed: {}", child);
    if child == 0 {
        let mapping = shmat(shmid as usize, 0, 0);
        assert!(mapping > 0, "child shmat failed: {}", mapping);
        write_word(mapping as usize, 0x519a_c0de);
        assert_eq!(shmctl(shmid as usize, IPC_RMID, 0), 0);
        assert_eq!(kill(getpid() as usize, SIGKILL), 0);
        panic!("SIGKILL returned to user mode");
    }

    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, SIGKILL);
    let gone = stale_id_is_gone(shmid as usize);

    let replacement = create_segment(key);
    assert_eq!(shmctl(replacement as usize, IPC_RMID, 0), 0);
    if gone {
        println!("SYSV_SHM_LIFECYCLE signal_exit_cleanup PASS");
    }
    gone
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_explicit_last_detach();
    let exit_cleanup = test_exit_without_shmdt();
    let exec_cleanup = test_exec_without_shmdt();
    test_fork_inherited_attachment();
    let signal_exit_cleanup = test_signal_exit_without_shmdt();

    if !exit_cleanup || !exec_cleanup || !signal_exit_cleanup {
        println!(
            "SYSV_SHM_LIFECYCLE_EXPECTED_FAIL exit_stale={} exec_stale={} signal_exit_stale={}",
            !exit_cleanup, !exec_cleanup, !signal_exit_cleanup
        );
        return 1;
    }

    println!("SYSV_SHM_LIFECYCLE PASS");
    0
}
