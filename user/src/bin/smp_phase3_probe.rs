#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null;
use user_lib::{exec, exit, fork, waitpid};

const ROUNDS: usize = 30;
const PARALLEL_PROBES: [&str; 6] = [
    "task_a_wait4_probe\0",
    "task_a_wait4_probe\0",
    "pipetest\0",
    "pipetest\0",
    "net_loopback_smoke\0",
    "net_loopback_smoke\0",
];

fn spawn_exec(path: &'static str) -> isize {
    let pid = fork();
    assert!(pid >= 0, "fork failed for {}", path);
    if pid == 0 {
        let argv = [path.as_ptr(), null()];
        let ret = exec(path, &argv);
        println!("[smp-phase3] exec {} failed: {}", path, ret);
        exit(127);
        unreachable!();
    }
    pid
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!(
        "[smp-phase3] start: {} rounds, {} concurrent probes",
        ROUNDS,
        PARALLEL_PROBES.len()
    );

    for round in 1..=ROUNDS {
        let mut pids = [0isize; PARALLEL_PROBES.len()];
        for (pid, path) in pids.iter_mut().zip(PARALLEL_PROBES) {
            *pid = spawn_exec(path);
        }

        for pid in pids {
            let mut status = 0;
            assert_eq!(waitpid(pid as usize, &mut status), pid);
            assert_eq!(status, 0, "child {} failed in round {}", pid, round);
        }
        println!("[smp-phase3] round {}/{} PASS", round, ROUNDS);
    }

    println!("SMP_PHASE3_PROBE_PASS");
    0
}
