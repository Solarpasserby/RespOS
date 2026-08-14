#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_INET, PollFd, SOCK_NONBLOCK, SOCK_STREAM, SockAddrIn, TimeSpec, accept_unix, bind, close,
    connect, getsockname, getsockopt_raw, listen, ppoll_raw, read, sendto, socket,
};

const SOL_SOCKET: usize = 1;
const SO_ERROR: usize = 4;
const POLLOUT: i16 = 0x0004;
const POLLERR: i16 = 0x0008;
const EINPROGRESS: isize = 115;
const ECONNREFUSED: i32 = 111;

fn read_so_error(fd: usize) -> i32 {
    let mut error = -1i32;
    let mut len = core::mem::size_of::<i32>() as u32;
    assert_eq!(
        getsockopt_raw(fd, SOL_SOCKET, SO_ERROR, &mut error, &mut len),
        0
    );
    assert_eq!(len as usize, core::mem::size_of::<i32>());
    error
}

fn wait_connect_ready(fd: i32) -> i16 {
    let mut pollfds = [PollFd {
        fd,
        events: POLLOUT,
        revents: 0,
    }];
    let timeout = TimeSpec { sec: 1, nsec: 0 };
    assert_eq!(ppoll_raw(&mut pollfds, &timeout, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLOUT, 0);
    pollfds[0].revents
}

fn bound_loopback_socket() -> (isize, SockAddrIn) {
    let fd = socket(AF_INET, SOCK_STREAM, 0);
    assert!(fd >= 0);
    let requested = SockAddrIn::loopback(0);
    assert_eq!(bind(fd as usize, &requested), 0);
    let mut bound = SockAddrIn::default();
    let mut bound_len = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(getsockname(fd as usize, &mut bound, &mut bound_len), 0);
    assert_eq!(bound_len as usize, core::mem::size_of::<SockAddrIn>());
    assert_ne!(bound.sin_port, 0);
    (fd, bound)
}

fn test_success() {
    let (listener, addr) = bound_loopback_socket();
    assert_eq!(listen(listener as usize, 4), 0);

    let client = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0);
    assert!(client >= 0);
    assert_eq!(connect(client as usize, &addr), -EINPROGRESS);
    let revents = wait_connect_ready(client as i32);
    assert_eq!(revents & POLLERR, 0);
    assert_eq!(read_so_error(client as usize), 0);
    assert_eq!(read_so_error(client as usize), 0);

    let accepted = accept_unix(listener as usize);
    assert!(accepted >= 0);
    assert_eq!(sendto(client as usize, b"s", 0, None), 1);
    let mut byte = [0u8; 1];
    assert_eq!(read(accepted as usize, &mut byte), 1);
    assert_eq!(byte[0], b's');
    assert_eq!(close(accepted as usize), 0);
    assert_eq!(close(client as usize), 0);
    assert_eq!(close(listener as usize), 0);
    println!("SOCKET_CONNECT success PASS");
}

fn test_refused_and_error_consumption() {
    let (reservation, addr) = bound_loopback_socket();
    assert_eq!(close(reservation as usize), 0);

    let client = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0);
    assert!(client >= 0);
    assert_eq!(connect(client as usize, &addr), -EINPROGRESS);
    let revents = wait_connect_ready(client as i32);
    assert_ne!(revents & POLLERR, 0);
    assert_eq!(read_so_error(client as usize), ECONNREFUSED);
    assert_eq!(read_so_error(client as usize), 0);
    let revents = wait_connect_ready(client as i32);
    assert_eq!(revents & POLLERR, 0);
    assert_eq!(close(client as usize), 0);
    println!("SOCKET_CONNECT refused_so_error PASS");
}

fn test_blocking_refused_has_no_pending_error() {
    let (reservation, addr) = bound_loopback_socket();
    assert_eq!(close(reservation as usize), 0);

    let client = socket(AF_INET, SOCK_STREAM, 0);
    assert!(client >= 0);
    assert_eq!(connect(client as usize, &addr), -(ECONNREFUSED as isize));
    assert_eq!(read_so_error(client as usize), 0);
    assert_eq!(close(client as usize), 0);
    println!("SOCKET_CONNECT blocking_refused PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_success();
    test_refused_and_error_consumption();
    test_blocking_refused_has_no_pending_error();
    println!("SOCKET_CONNECT ALL PASS");
    0
}
