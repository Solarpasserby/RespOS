#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_INET, PollFd, SOCK_STREAM, SockAddrIn, TimeSpec, accept_unix, bind, close, connect, dup,
    epoll_create1, epoll_ctl, epoll_pwait, getsockname, listen, ppoll_raw, read, sendto, shutdown,
    socket, yield_,
};

const SHUT_WR: usize = 1;
const MSG_NOSIGNAL: usize = 0x4000;
const EINVAL: isize = 22;
const EPIPE: isize = 32;
const ENOTCONN: isize = 107;
const POLLIN: i16 = 0x0001;
const POLLRDHUP: i16 = 0x2000;
const EPOLL_CTL_ADD: usize = 1;

fn make_listener() -> (usize, SockAddrIn) {
    let fd = socket(AF_INET, SOCK_STREAM, 0);
    assert!(fd >= 0);
    let requested = SockAddrIn::loopback(0);
    assert_eq!(bind(fd as usize, &requested), 0);
    assert_eq!(listen(fd as usize, 4), 0);

    let mut addr = SockAddrIn::default();
    let mut addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(getsockname(fd as usize, &mut addr, &mut addrlen), 0);
    assert_eq!(addrlen as usize, core::mem::size_of::<SockAddrIn>());
    assert_ne!(addr.sin_port, 0);
    (fd as usize, addr)
}

fn receive_exact(fd: usize, expected: &[u8]) {
    let mut buffer = [0u8; 16];
    assert!(expected.len() <= buffer.len());
    let mut received = 0;
    while received < expected.len() {
        let result = read(fd, &mut buffer[received..expected.len()]);
        assert!(result > 0);
        received += result as usize;
    }
    assert_eq!(&buffer[..expected.len()], expected);
}

fn expect_readable(fd: usize) {
    let mut pollfds = [PollFd {
        fd: fd as i32,
        events: POLLIN,
        revents: 0,
    }];
    let timeout = TimeSpec { sec: 1, nsec: 0 };
    assert_eq!(ppoll_raw(&mut pollfds, &timeout, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLIN, 0);
}

fn expect_rdhup_with_buffered_data(fd: usize) {
    let zero = TimeSpec { sec: 0, nsec: 0 };
    let mut pollfds = [PollFd {
        fd: fd as i32,
        events: POLLIN | POLLRDHUP,
        revents: 0,
    }];
    for _ in 0..100 {
        pollfds[0].revents = 0;
        assert!(ppoll_raw(&mut pollfds, &zero, core::ptr::null(), 0) >= 0);
        if pollfds[0].revents & POLLRDHUP != 0 {
            break;
        }
        yield_();
    }
    assert_ne!(pollfds[0].revents & POLLIN, 0);
    assert_ne!(pollfds[0].revents & POLLRDHUP, 0);

    let epfd = epoll_create1(0);
    assert!(epfd >= 0);
    let epfd = epfd as usize;
    let mut interest = [0u8; 12];
    interest[..4].copy_from_slice(&((POLLIN | POLLRDHUP) as u32).to_ne_bytes());
    interest[4..].copy_from_slice(&0x5244485550u64.to_ne_bytes());
    assert_eq!(epoll_ctl(epfd, EPOLL_CTL_ADD, fd, interest.as_ptr()), 0);
    let mut ready = [0u8; 12];
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 0, core::ptr::null(), 0),
        1
    );
    let events = u32::from_ne_bytes(ready[..4].try_into().unwrap());
    assert_ne!(events & POLLIN as u32, 0);
    assert_ne!(events & POLLRDHUP as u32, 0);
    assert_eq!(
        u64::from_ne_bytes(ready[4..].try_into().unwrap()),
        0x5244485550
    );
    assert_eq!(close(epfd), 0);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let unconnected = socket(AF_INET, SOCK_STREAM, 0);
    assert!(unconnected >= 0);
    assert_eq!(shutdown(unconnected as usize, SHUT_WR), -ENOTCONN);
    assert_eq!(close(unconnected as usize), 0);

    let (listener, addr) = make_listener();
    let client = socket(AF_INET, SOCK_STREAM, 0);
    assert!(client >= 0);
    let client = client as usize;
    assert_eq!(connect(client, &addr), 0);
    let server = accept_unix(listener);
    assert!(server >= 0);
    let server = server as usize;

    assert_eq!(shutdown(client, 3), -EINVAL);
    assert_eq!(sendto(client, b"request", 0, None), 7);

    let duplicate = dup(client);
    assert!(duplicate >= 0);
    let duplicate = duplicate as usize;
    assert_eq!(shutdown(client, SHUT_WR), 0);
    assert_eq!(shutdown(client, SHUT_WR), 0);
    assert_eq!(sendto(duplicate, b"x", MSG_NOSIGNAL, None), -EPIPE);

    expect_rdhup_with_buffered_data(server);
    receive_exact(server, b"request");
    let mut byte = [0u8; 1];
    expect_readable(server);
    assert_eq!(read(server, &mut byte), 0);
    assert_eq!(sendto(server, b"response", 0, None), 8);
    receive_exact(duplicate, b"response");

    assert_eq!(shutdown(server, SHUT_WR), 0);
    expect_readable(client);
    assert_eq!(read(client, &mut byte), 0);

    assert_eq!(close(duplicate), 0);
    assert_eq!(close(client), 0);
    assert_eq!(close(server), 0);
    assert_eq!(close(listener), 0);
    println!(
        "TCP_HALF_CLOSE PASS errors=pass queued_fin=pass reverse_flow=pass dup=pass poll_eof=pass rdhup=pass"
    );
    0
}
