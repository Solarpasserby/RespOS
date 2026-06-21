#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::mem::size_of;
use user_lib::{
    accept, bind, close, connect, exit, fork, listen, read, recvfrom, sendto, socket, wait, write,
    SockAddrIn, AF_INET, IPPROTO_TCP, IPPROTO_UDP, SOCK_DGRAM, SOCK_STREAM,
};

const UDP_PORT: u16 = 41000;
const TCP_PORT: u16 = 41001;

fn udp_loopback() {
    let server_addr = SockAddrIn::loopback(UDP_PORT);
    let server = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    assert!(server >= 0);
    assert_eq!(bind(server as usize, &server_addr), 0);

    let client = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    assert!(client >= 0);

    let message = b"udp-loopback";
    assert_eq!(
        sendto(client as usize, message, 0, Some(&server_addr)),
        message.len() as isize
    );

    let mut buf = [0u8; 32];
    let mut from = SockAddrIn::default();
    let mut from_len = size_of::<SockAddrIn>() as u32;
    let n = recvfrom(
        server as usize,
        &mut buf,
        0,
        Some((&mut from, &mut from_len)),
    );
    assert_eq!(n, message.len() as isize);
    assert_eq!(&buf[..n as usize], message);
    assert_eq!(from.sin_addr, [127, 0, 0, 1]);
    assert!(u16::from_be(from.sin_port) != 0);

    close(client as usize);
    close(server as usize);
}

fn tcp_loopback() {
    let server_addr = SockAddrIn::loopback(TCP_PORT);
    let server = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    assert!(server >= 0);
    assert_eq!(bind(server as usize, &server_addr), 0);
    assert_eq!(listen(server as usize, 8), 0);

    let pid = fork();
    assert!(pid >= 0);
    if pid == 0 {
        let client = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        assert!(client >= 0);
        assert_eq!(connect(client as usize, &server_addr), 0);

        let request = b"tcp-loopback";
        assert_eq!(write(client as usize, request), request.len() as isize);

        let mut reply = [0u8; 32];
        let n = read(client as usize, &mut reply);
        assert_eq!(n, request.len() as isize);
        assert_eq!(&reply[..n as usize], request);
        close(client as usize);
        exit(0);
        unreachable!();
    }

    let mut peer = SockAddrIn::default();
    let mut peer_len = size_of::<SockAddrIn>() as u32;
    let accepted = accept(server as usize, &mut peer, &mut peer_len);
    assert!(accepted >= 0);
    assert_eq!(peer.sin_addr, [127, 0, 0, 1]);

    let mut request = [0u8; 32];
    let n = read(accepted as usize, &mut request);
    assert_eq!(n, b"tcp-loopback".len() as isize);
    assert_eq!(&request[..n as usize], b"tcp-loopback");
    assert_eq!(write(accepted as usize, &request[..n as usize]), n);

    close(accepted as usize);
    close(server as usize);

    let mut child_exit_code = 0;
    wait(&mut child_exit_code);
    assert_eq!(child_exit_code, 0);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    udp_loopback();
    tcp_loopback();
    println!("net_loopback_smoke passed!");
    0
}
