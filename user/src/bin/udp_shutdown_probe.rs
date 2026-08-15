#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_INET, SOCK_DGRAM, SockAddrIn, bind, close, connect, getsockname, recvfrom, sendto, shutdown,
    socket,
};

const SHUT_RD: usize = 0;
const SHUT_WR: usize = 1;
const SHUT_RDWR: usize = 2;
const MSG_NOSIGNAL: usize = 0x4000;
const EINVAL: isize = 22;
const EPIPE: isize = 32;
const ENOTCONN: isize = 107;

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
    expect_send_epipe(pair.0);
    send_and_receive(pair.1, pair.0, b"reverse");
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
    assert_eq!(recvfrom(pair.0, &mut byte, 0, None), 0);
    expect_send_epipe(pair.0);
    close_pair(pair);

    println!("UDP_SHUTDOWN PASS unconnected=pass shut_wr=pass shut_rd=pass shut_rdwr=pass");
    0
}
