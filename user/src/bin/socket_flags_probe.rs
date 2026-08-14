#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    AF_UNIX, SIGPIPE, SIGUSR1, SOCK_STREAM, SignalAction, TimeSpec, close, exit, fork, getpid,
    kill, nanosleep, recvfrom, sendto, setsockopt_raw, shutdown, sigaction, socketpair, waitpid,
};

const SOL_SOCKET: usize = 1;
const SO_RCVTIMEO: usize = 20;
const MSG_PEEK: usize = 0x2;
const MSG_WAITALL: usize = 0x100;
const MSG_NOSIGNAL: usize = 0x4000;
const EPIPE: isize = 32;
const SHUT_WR: usize = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct SocketTimeval {
    sec: isize,
    usec: isize,
}

static SIGPIPE_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGUSR1_COUNT: AtomicUsize = AtomicUsize::new(0);

fn sigpipe_handler() {
    SIGPIPE_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn sigusr1_handler() {
    SIGUSR1_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn sleep_ms(ms: usize) {
    let request = TimeSpec {
        sec: ms / 1000,
        nsec: (ms % 1000) * 1_000_000,
    };
    let mut remaining = TimeSpec::default();
    assert_eq!(nanosleep(&request, &mut remaining), 0);
}

fn wait_child(child: isize) {
    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
}

fn test_peek() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(sendto(fds[0] as usize, b"abc", 0, None), 3);

    let mut peeked = [0u8; 2];
    assert_eq!(recvfrom(fds[1] as usize, &mut peeked, MSG_PEEK, None), 2);
    assert_eq!(&peeked, b"ab");

    let mut received = [0u8; 3];
    assert_eq!(recvfrom(fds[1] as usize, &mut received, 0, None), 3);
    assert_eq!(&received, b"abc");
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("SOCKET_FLAGS MSG_PEEK PASS");
}

fn test_waitall_fragmented() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(fds[0] as usize), 0);
        assert_eq!(sendto(fds[1] as usize, b"ab", 0, None), 2);
        sleep_ms(50);
        assert_eq!(sendto(fds[1] as usize, b"cd", 0, None), 2);
        assert_eq!(close(fds[1] as usize), 0);
        exit(0);
        unreachable!();
    }

    assert_eq!(close(fds[1] as usize), 0);
    let mut received = [0u8; 4];
    assert_eq!(
        recvfrom(fds[0] as usize, &mut received, MSG_WAITALL, None),
        4
    );
    assert_eq!(&received, b"abcd");
    wait_child(child);
    assert_eq!(close(fds[0] as usize), 0);
    println!("SOCKET_FLAGS MSG_WAITALL fragmented PASS");
}

fn test_waitall_partial_timeout() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let timeout = SocketTimeval {
        sec: 0,
        usec: 50_000,
    };
    assert_eq!(
        setsockopt_raw(fds[1] as usize, SOL_SOCKET, SO_RCVTIMEO, &timeout),
        0
    );
    assert_eq!(sendto(fds[0] as usize, b"xy", 0, None), 2);

    let mut received = [0u8; 4];
    assert_eq!(
        recvfrom(fds[1] as usize, &mut received, MSG_WAITALL, None),
        2
    );
    assert_eq!(&received[..2], b"xy");
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("SOCKET_FLAGS MSG_WAITALL timeout partial PASS");
}

fn test_waitall_partial_eof() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(sendto(fds[0] as usize, b"pq", 0, None), 2);
    assert_eq!(shutdown(fds[0] as usize, SHUT_WR), 0);

    let mut received = [0u8; 4];
    assert_eq!(
        recvfrom(fds[1] as usize, &mut received, MSG_WAITALL, None),
        2
    );
    assert_eq!(&received[..2], b"pq");
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("SOCKET_FLAGS MSG_WAITALL EOF partial PASS");
}

fn test_waitall_partial_signal() {
    let action = SignalAction {
        handler: sigusr1_handler as usize,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&action), None), 0);

    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let parent = getpid();
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(fds[0] as usize), 0);
        assert_eq!(sendto(fds[1] as usize, b"uv", 0, None), 2);
        sleep_ms(50);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        sleep_ms(50);
        assert_eq!(close(fds[1] as usize), 0);
        exit(0);
        unreachable!();
    }

    assert_eq!(close(fds[1] as usize), 0);
    let mut received = [0u8; 4];
    assert_eq!(
        recvfrom(fds[0] as usize, &mut received, MSG_WAITALL, None),
        2
    );
    assert_eq!(&received[..2], b"uv");
    assert_eq!(SIGUSR1_COUNT.load(Ordering::SeqCst), 1);
    wait_child(child);
    assert_eq!(close(fds[0] as usize), 0);
    println!("SOCKET_FLAGS MSG_WAITALL signal partial PASS");
}

fn test_nosignal() {
    let action = SignalAction {
        handler: sigpipe_handler as usize,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGPIPE, Some(&action), None), 0);

    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(close(fds[1] as usize), 0);
    assert_eq!(sendto(fds[0] as usize, b"x", 0, None), -EPIPE);
    assert_eq!(SIGPIPE_COUNT.load(Ordering::SeqCst), 1);
    assert_eq!(close(fds[0] as usize), 0);

    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(close(fds[1] as usize), 0);
    assert_eq!(sendto(fds[0] as usize, b"x", MSG_NOSIGNAL, None), -EPIPE);
    assert_eq!(SIGPIPE_COUNT.load(Ordering::SeqCst), 1);
    assert_eq!(close(fds[0] as usize), 0);
    println!("SOCKET_FLAGS MSG_NOSIGNAL PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_peek();
    test_waitall_fragmented();
    test_waitall_partial_timeout();
    test_waitall_partial_eof();
    test_waitall_partial_signal();
    test_nosignal();
    println!("SOCKET_FLAGS ALL PASS");
    0
}
