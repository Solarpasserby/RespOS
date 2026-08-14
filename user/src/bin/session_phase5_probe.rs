#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, exit, fork, getpgid, getpid, getsid, pipe, read, setpgid, setsid, waitpid, write,
};

const EPERM: isize = 1;
const ESRCH: isize = 3;

fn wait_child(child: isize) {
    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
}

fn test_getsid_query_and_errors() {
    let self_pid = getpid();
    let sid = getsid(0);
    assert!(sid > 0);
    assert_eq!(getsid(self_pid), sid);
    assert_eq!(getsid(-1), -ESRCH);
    assert_eq!(getsid(1 << 29), -ESRCH);
    println!("SESSION_PHASE5_RESPOS query_errors PASS");
}

fn test_child_setsid_and_parent_query() {
    let mut ready = [-1i32; 2];
    let mut release = [-1i32; 2];
    assert_eq!(pipe(&mut ready), 0);
    assert_eq!(pipe(&mut release), 0);

    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(ready[0] as usize), 0);
        assert_eq!(close(release[1] as usize), 0);
        let sid = setsid();
        assert_eq!(sid, getpid());
        assert_eq!(getsid(0), sid);
        assert_eq!(write(ready[1] as usize, &sid.to_ne_bytes()), 8);
        let mut byte = [0u8; 1];
        assert_eq!(read(release[0] as usize, &mut byte), 1);
        exit(0);
        unreachable!();
    }

    assert_eq!(close(ready[1] as usize), 0);
    assert_eq!(close(release[0] as usize), 0);
    let mut sid_bytes = [0u8; 8];
    assert_eq!(read(ready[0] as usize, &mut sid_bytes), 8);
    let child_sid = isize::from_ne_bytes(sid_bytes);
    assert_eq!(child_sid, child);
    assert_eq!(getsid(child), child_sid);
    assert_eq!(write(release[1] as usize, b"x"), 1);
    wait_child(child);
    assert_eq!(close(ready[0] as usize), 0);
    assert_eq!(close(release[1] as usize), 0);
    println!("SESSION_PHASE5_RESPOS child_setsid_parent_query PASS");
}

fn test_process_group_leader_cannot_setsid() {
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(setpgid(0, 0), 0);
        assert_eq!(getpgid(0), getpid());
        assert_eq!(setsid(), -EPERM);
        exit(0);
        unreachable!();
    }
    wait_child(child);
    println!("SESSION_PHASE5_RESPOS pgrp_leader_setsid_eperm PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_getsid_query_and_errors();
    test_child_setsid_and_parent_query();
    test_process_group_leader_cannot_setsid();
    println!("SESSION_PHASE5_RESPOS ALL PASS");
    0
}
