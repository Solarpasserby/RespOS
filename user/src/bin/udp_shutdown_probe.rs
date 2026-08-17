#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_INET, EPOLL_DATA_OFFSET, EPOLL_EVENT_SIZE, PollFd, SOCK_DGRAM, SockAddrIn, TimeSpec, bind,
    close, connect, epoll_create1, epoll_ctl, epoll_pwait, exit, fork, getsockname, ppoll_raw,
    recvfrom, sendto, shutdown, socket, time_get, waitpid, yield_,
};

const SHUT_RD: usize = 0;
const SHUT_WR: usize = 1;
const SHUT_RDWR: usize = 2;
const MSG_NOSIGNAL: usize = 0x4000;
const EINVAL: isize = 22;
const EPIPE: isize = 32;
const ENOTCONN: isize = 107;
const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
const POLLHUP: i16 = 0x0010;
const POLLRDHUP: i16 = 0x2000;
const EPOLL_CTL_ADD: usize = 1;

fn bind_loopback(fd: usize) -> SockAddrIn {
    assert_eq!(bind(fd, &SockAddrIn::loopback(0)), 0);
    let mut addr = SockAddrIn::default();
    let mut addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(getsockname(fd, &mut addr, &mut addrlen), 0);
    assert_eq!(addrlen as usize, core::mem::size_of::<SockAddrIn>());
    assert_ne!(addr.sin_port, 0);
    addr
}

fn make_pair() -> (usize, usize) {
    let left = socket(AF_INET, SOCK_DGRAM, 0);
    let right = socket(AF_INET, SOCK_DGRAM, 0);
    assert!(left >= 0 && right >= 0);
    let left = left as usize;
    let right = right as usize;
    let left_addr = bind_loopback(left);
    let right_addr = bind_loopback(right);
    assert_eq!(connect(left, &right_addr), 0);
    assert_eq!(connect(right, &left_addr), 0);
    (left, right)
}

fn close_pair(pair: (usize, usize)) {
    assert_eq!(close(pair.0), 0);
    assert_eq!(close(pair.1), 0);
}

fn send_and_receive(sender: usize, receiver: usize, payload: &[u8]) {
    assert_eq!(sendto(sender, payload, 0, None), payload.len() as isize);
    let mut buffer = [0u8; 16];
    assert!(payload.len() <= buffer.len());
    assert_eq!(
        recvfrom(receiver, &mut buffer, 0, None),
        payload.len() as isize
    );
    assert_eq!(&buffer[..payload.len()], payload);
}

fn expect_send_epipe(fd: usize) {
    assert_eq!(sendto(fd, b"x", MSG_NOSIGNAL, None), -EPIPE);
}

fn expect_readiness(fd: usize, expected: i16) {
    let mut pollfds = [PollFd {
        fd: fd as i32,
        events: POLLIN | POLLOUT | POLLRDHUP,
        revents: 0,
    }];
    let zero = TimeSpec { sec: 0, nsec: 0 };
    assert_eq!(ppoll_raw(&mut pollfds, &zero, core::ptr::null(), 0), 1);
    assert_eq!(pollfds[0].revents, expected);

    let epfd = epoll_create1(0);
    assert!(epfd >= 0);
    let epfd = epfd as usize;
    let mut interest = [0u8; EPOLL_EVENT_SIZE];
    interest[..4].copy_from_slice(&((POLLIN | POLLOUT | POLLRDHUP) as u32).to_ne_bytes());
    interest[EPOLL_DATA_OFFSET..].copy_from_slice(&0x55445053485554u64.to_ne_bytes());
    assert_eq!(epoll_ctl(epfd, EPOLL_CTL_ADD, fd, interest.as_ptr()), 0);
    let mut ready = [0u8; EPOLL_EVENT_SIZE];
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 0, core::ptr::null(), 0),
        1
    );
    assert_eq!(
        u32::from_ne_bytes(ready[..4].try_into().unwrap()),
        expected as u32
    );
    assert_eq!(
        u64::from_ne_bytes(ready[EPOLL_DATA_OFFSET..].try_into().unwrap()),
        0x55445053485554
    );
    assert_eq!(close(epfd), 0);
}

fn delay_ms(ms: isize) {
    let deadline = time_get().saturating_add(ms);
    while time_get() < deadline {
        let _ = yield_();
    }
}

fn expect_blocking_poll_shutdown() {
    let pair = make_pair();
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(100);
        exit(if shutdown(pair.0, SHUT_RD) == 0 { 0 } else { 1 });
    }

    let mut pollfds = [PollFd {
        fd: pair.0 as i32,
        events: POLLIN | POLLRDHUP,
        revents: 0,
    }];
    let timeout = TimeSpec { sec: 2, nsec: 0 };
    assert_eq!(ppoll_raw(&mut pollfds, &timeout, core::ptr::null(), 0), 1);
    assert_eq!(pollfds[0].revents, POLLIN | POLLRDHUP);
    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    close_pair(pair);

    let pair = make_pair();
    let epfd = epoll_create1(0);
    assert!(epfd >= 0);
    let epfd = epfd as usize;
    let mut interest = [0u8; EPOLL_EVENT_SIZE];
    interest[..4].copy_from_slice(&((POLLIN | POLLRDHUP) as u32).to_ne_bytes());
    interest[EPOLL_DATA_OFFSET..].copy_from_slice(&0x554450424c4f43u64.to_ne_bytes());
    assert_eq!(epoll_ctl(epfd, EPOLL_CTL_ADD, pair.0, interest.as_ptr()), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(100);
        exit(if shutdown(pair.0, SHUT_RD) == 0 { 0 } else { 1 });
    }
    let mut ready = [0u8; EPOLL_EVENT_SIZE];
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 2000, core::ptr::null(), 0),
        1
    );
    assert_eq!(
        u32::from_ne_bytes(ready[..4].try_into().unwrap()),
        (POLLIN | POLLRDHUP) as u32
    );
    assert_eq!(
        u64::from_ne_bytes(ready[EPOLL_DATA_OFFSET..].try_into().unwrap()),
        0x554450424c4f43
    );
    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(epfd), 0);
    close_pair(pair);
}

fn sentinel_sockaddr() -> SockAddrIn {
    SockAddrIn {
        sin_family: 0xa5a5,
        sin_port: 0xa5a5,
        sin_addr: [0xa5; 4],
        sin_zero: [0xa5; 8],
    }
}

fn expect_zero_datagram_source() {
    let pair = make_pair();
    assert_eq!(sendto(pair.1, b"", 0, None), 0);
    let mut byte = [0xccu8; 1];
    let mut from = sentinel_sockaddr();
    let mut from_len = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(
        recvfrom(pair.0, &mut byte, 0, Some((&mut from, &mut from_len))),
        0
    );
    assert_eq!(byte[0], 0xcc);
    assert_eq!(from_len as usize, core::mem::size_of::<SockAddrIn>());
    assert_eq!(from.sin_family as usize, AF_INET);
    assert_ne!(from.sin_port, 0);
    close_pair(pair);
}

fn expect_blocked_recv_shutdown() {
    let pair = make_pair();
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(100);
        exit(if shutdown(pair.0, SHUT_RD) == 0 { 0 } else { 1 });
    }

    let mut byte = [0xccu8; 1];
    let mut from = sentinel_sockaddr();
    let mut from_len = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(
        recvfrom(pair.0, &mut byte, 0, Some((&mut from, &mut from_len))),
        0
    );
    assert_eq!(byte[0], 0xcc);
    assert_eq!(from_len, 0);
    assert_eq!(from.sin_family, 0xa5a5);
    let mut status = 0;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    close_pair(pair);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let unconnected = socket(AF_INET, SOCK_DGRAM, 0);
    assert!(unconnected >= 0);
    let unconnected = unconnected as usize;
    assert_eq!(shutdown(unconnected, SHUT_RD), -ENOTCONN);
    assert_eq!(shutdown(unconnected, SHUT_WR), -ENOTCONN);
    assert_eq!(shutdown(unconnected, SHUT_RDWR), -ENOTCONN);
    assert_eq!(close(unconnected), 0);

    let bound = socket(AF_INET, SOCK_DGRAM, 0);
    assert!(bound >= 0);
    let bound = bound as usize;
    let _ = bind_loopback(bound);
    assert_eq!(shutdown(bound, SHUT_RDWR), -ENOTCONN);
    assert_eq!(close(bound), 0);

    let pair = make_pair();
    assert_eq!(shutdown(pair.0, 3), -EINVAL);
    send_and_receive(pair.0, pair.1, b"before");
    assert_eq!(shutdown(pair.0, SHUT_WR), 0);
    assert_eq!(shutdown(pair.0, SHUT_WR), 0);
    expect_readiness(pair.0, POLLOUT);
    expect_send_epipe(pair.0);
    send_and_receive(pair.1, pair.0, b"reverse");
    close_pair(pair);

    let pair = make_pair();
    assert_eq!(shutdown(pair.0, SHUT_RD), 0);
    expect_readiness(pair.0, POLLIN | POLLOUT | POLLRDHUP);
    close_pair(pair);

    let pair = make_pair();
    assert_eq!(sendto(pair.1, b"drop", 0, None), 4);
    assert_eq!(shutdown(pair.0, SHUT_RD), 0);
    let mut byte = [0u8; 1];
    assert_eq!(recvfrom(pair.0, &mut byte, 0, None), 1);
    assert_eq!(byte[0], b'd');
    assert_eq!(recvfrom(pair.0, &mut byte, 0, None), 0);
    send_and_receive(pair.0, pair.1, b"out");
    send_and_receive(pair.1, pair.0, b"future");
    assert_eq!(recvfrom(pair.0, &mut byte, 0, None), 0);
    close_pair(pair);

    let pair = make_pair();
    assert_eq!(shutdown(pair.0, SHUT_RDWR), 0);
    expect_readiness(pair.0, POLLIN | POLLOUT | POLLHUP | POLLRDHUP);
    assert_eq!(recvfrom(pair.0, &mut byte, 0, None), 0);
    expect_send_epipe(pair.0);
    close_pair(pair);

    expect_blocking_poll_shutdown();
    expect_zero_datagram_source();
    expect_blocked_recv_shutdown();

    println!(
        "UDP_SHUTDOWN PASS unconnected=pass shut_wr=pass shut_rd=pass shut_rdwr=pass readiness=pass blocking_poll=pass blocking_epoll=pass zero_datagram=pass blocked_recv=pass eof_addr=pass"
    );
    0
}
