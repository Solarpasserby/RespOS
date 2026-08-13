#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::mem::size_of;
use user_lib::{
    AF_INET, IPPROTO_TCP, IPPROTO_UDP, PollFd, SIGKILL, SOCK_DGRAM, SOCK_STREAM, SockAddrIn,
    TimeSpec, accept, bind, close, exit, fork, getpid, kill, listen, nanosleep, pipe, ppoll_raw,
    read, socket, waitpid, write, yield_,
};

const PORT_BASE: u16 = 47000;
const PORT_SLOTS: u16 = 1000;

fn spawn_accept_waiter(port: u16, ready_fd: usize) -> isize {
    let pid = fork();
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        assert!(listener >= 0, "socket failed: {}", listener);
        assert_eq!(bind(listener as usize, &SockAddrIn::loopback(port)), 0);
        assert_eq!(listen(listener as usize, 4), 0);
        assert_eq!(write(ready_fd, &[1]), 1);

        let mut peer = SockAddrIn::default();
        let mut peer_len = size_of::<SockAddrIn>() as u32;
        let accepted = accept(listener as usize, &mut peer, &mut peer_len);
        if accepted >= 0 {
            close(accepted as usize);
        }
        close(listener as usize);
        exit(0);
        unreachable!();
    }
    pid
}

fn spawn_udp_waiter(port: u16, ready_fd: usize) -> isize {
    let pid = fork();
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        let receiver = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
        assert!(receiver >= 0, "socket failed: {}", receiver);
        assert_eq!(bind(receiver as usize, &SockAddrIn::loopback(port)), 0);
        assert_eq!(write(ready_fd, &[1]), 1);

        // iperf3's daemon waits for network readiness through poll(2). Inet
        // sockets currently use the polling fallback instead of registering
        // an event-driven FileOp waiter, so this remains inside one syscall.
        let mut pollfd = [PollFd {
            fd: receiver as i32,
            events: 1,
            revents: 0,
        }];
        let received = ppoll_raw(&mut pollfd, core::ptr::null(), core::ptr::null(), 0);
        close(receiver as usize);
        exit(if received >= 0 { 0 } else { 1 });
        unreachable!();
    }
    pid
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let mut ready = [0i32; 2];
    assert_eq!(pipe(&mut ready), 0);

    let slot = (getpid() as u16 % PORT_SLOTS) * 2;
    let first = spawn_accept_waiter(PORT_BASE + slot, ready[1] as usize);
    let second = spawn_udp_waiter(PORT_BASE + slot + 1, ready[1] as usize);
    close(ready[1] as usize);

    let mut byte = [0u8; 1];
    assert_eq!(read(ready[0] as usize, &mut byte), 1);
    assert_eq!(read(ready[0] as usize, &mut byte), 1);
    close(ready[0] as usize);

    // Give the TCP accept and UDP poll paths time to enter their blocking
    // retry loops. A permanently runnable network waiter must not starve an
    // unrelated process's global nanosleep deadline.
    for _ in 0..64 {
        yield_();
    }

    let request = TimeSpec {
        sec: 0,
        nsec: 100_000_000,
    };
    let mut remaining = TimeSpec::default();
    assert_eq!(nanosleep(&request, &mut remaining), 0);
    println!("NET_TIMER_PROGRESS_WAKE");

    assert_eq!(kill(first as usize, SIGKILL), 0);
    assert_eq!(kill(second as usize, SIGKILL), 0);
    let mut status = 0;
    assert_eq!(waitpid(first as usize, &mut status), first);
    assert_eq!(waitpid(second as usize, &mut status), second);
    println!("NET_TIMER_PROGRESS_PROBE_PASS");
    0
}
