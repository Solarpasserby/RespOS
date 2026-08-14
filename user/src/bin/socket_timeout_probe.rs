#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_UNIX, SOCK_STREAM, TimeSpec, close, exit, fork, getsockopt_raw, nanosleep, recvfrom, sendto,
    setsockopt_raw, setsockopt_with_len, socketpair, time_get, waitpid, write,
};

const SOL_SOCKET: usize = 1;
const SO_SNDBUF: usize = 7;
const SO_RCVTIMEO: usize = 20;
const SO_SNDTIMEO: usize = 21;
const MSG_DONTWAIT: usize = 0x40;
const MSG_NOSIGNAL: usize = 0x4000;
const EAGAIN: isize = 11;
const EINVAL: isize = 22;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SocketTimeval {
    sec: isize,
    usec: isize,
}

fn sleep_ms(ms: usize) {
    let request = TimeSpec {
        sec: ms / 1000,
        nsec: (ms % 1000) * 1_000_000,
    };
    let mut remaining = TimeSpec::default();
    assert_eq!(nanosleep(&request, &mut remaining), 0);
}

fn elapsed_ms(start: isize) -> isize {
    time_get().saturating_sub(start)
}

fn wait_child(child: isize) {
    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
}

fn test_recv_timeout_and_timeval_abi() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);

    let timeout = SocketTimeval {
        sec: 0,
        usec: 50_000,
    };
    assert_eq!(
        setsockopt_raw(fds[0] as usize, SOL_SOCKET, SO_RCVTIMEO, &timeout),
        0
    );
    let mut observed = SocketTimeval::default();
    let mut observed_len = core::mem::size_of::<SocketTimeval>() as u32;
    assert_eq!(
        getsockopt_raw(
            fds[0] as usize,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &mut observed,
            &mut observed_len,
        ),
        0
    );
    assert_eq!(observed_len as usize, core::mem::size_of::<SocketTimeval>());
    assert_eq!(observed, timeout);

    let invalid = SocketTimeval { sec: -1, usec: 0 };
    assert_eq!(
        setsockopt_raw(fds[0] as usize, SOL_SOCKET, SO_RCVTIMEO, &invalid),
        -EINVAL
    );
    assert_eq!(
        setsockopt_with_len(
            fds[0] as usize,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout,
            core::mem::size_of::<SocketTimeval>() - 1,
        ),
        -EINVAL
    );

    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(fds[0] as usize), 0);
        sleep_ms(200);
        assert_eq!(write(fds[1] as usize, b"x"), 1);
        exit(0);
        unreachable!();
    }
    assert_eq!(close(fds[1] as usize), 0);

    let mut byte = [0u8; 1];
    let start = time_get();
    assert_eq!(recvfrom(fds[0] as usize, &mut byte, 0, None), -EAGAIN);
    let elapsed = elapsed_ms(start);
    assert!((35..180).contains(&elapsed), "recv elapsed={}ms", elapsed);

    wait_child(child);
    assert_eq!(close(fds[0] as usize), 0);
    println!("SOCKET_TIMEOUT_RESPOS recv_timeout_timeval PASS");
}

fn test_zero_timeout_and_msg_dontwait() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let zero = SocketTimeval::default();
    assert_eq!(
        setsockopt_raw(fds[0] as usize, SOL_SOCKET, SO_RCVTIMEO, &zero),
        0
    );

    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(fds[0] as usize), 0);
        sleep_ms(50);
        assert_eq!(write(fds[1] as usize, b"z"), 1);
        exit(0);
        unreachable!();
    }
    assert_eq!(close(fds[1] as usize), 0);

    let mut byte = [0u8; 1];
    let start = time_get();
    assert_eq!(
        recvfrom(fds[0] as usize, &mut byte, MSG_DONTWAIT, None),
        -EAGAIN
    );
    assert!(elapsed_ms(start) < 30);
    assert_eq!(recvfrom(fds[0] as usize, &mut byte, 0, None), 1);
    assert_eq!(byte[0], b'z');

    wait_child(child);
    assert_eq!(close(fds[0] as usize), 0);
    println!("SOCKET_TIMEOUT_RESPOS zero_and_dontwait PASS");
}

fn test_send_timeout() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);

    let small_buffer = 4096i32;
    assert_eq!(
        setsockopt_raw(fds[0] as usize, SOL_SOCKET, SO_SNDBUF, &small_buffer),
        0
    );
    let timeout = SocketTimeval {
        sec: 0,
        usec: 50_000,
    };
    assert_eq!(
        setsockopt_raw(fds[0] as usize, SOL_SOCKET, SO_SNDTIMEO, &timeout),
        0
    );

    let payload = [0x5au8; 4096];
    let mut total = 0usize;
    loop {
        let start = time_get();
        let written = sendto(fds[0] as usize, &payload, MSG_NOSIGNAL, None);
        let elapsed = elapsed_ms(start);
        if written >= 0 {
            total += written as usize;
            continue;
        }
        assert_eq!(written, -EAGAIN);
        assert!((35..180).contains(&elapsed), "send elapsed={}ms", elapsed);
        break;
    }
    assert!(total > 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("SOCKET_TIMEOUT_RESPOS send_timeout PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_recv_timeout_and_timeval_abi();
    test_zero_timeout_and_msg_dontwait();
    test_send_timeout();
    println!("SOCKET_TIMEOUT_RESPOS ALL PASS");
    0
}
