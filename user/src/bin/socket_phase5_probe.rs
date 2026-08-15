#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    AF_UNIX, PollFd, SIGPIPE, SIGUSR1, SOCK_NONBLOCK, SOCK_STREAM, SignalAction, SockAddrUn,
    accept_unix, bind_unix, close, connect_unix, epoll_create1, epoll_ctl, epoll_pwait, exit, fork,
    getpid, kill, listen, pipe, ppoll_raw, read, shutdown, sigaction, socket, socketpair, time_get,
    unlink, waitpid, write, yield_,
};

const EAGAIN: isize = 11;
const EINTR: isize = 4;
const ECONNREFUSED: isize = 111;
const EPIPE: isize = 32;
const SHUT_WR: usize = 1;
const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
const POLLRDHUP: i16 = 0x2000;
const EPOLL_CTL_ADD: usize = 1;
static SIGNAL_SEEN: AtomicUsize = AtomicUsize::new(0);

fn signal_handler() {
    SIGNAL_SEEN.store(1, Ordering::SeqCst);
}

fn delay_ms(ms: isize) {
    let deadline = time_get().saturating_add(ms);
    while time_get() < deadline {
        let _ = yield_();
    }
}

fn unix_addr(path: &str) -> (SockAddrUn, usize) {
    SockAddrUn::from_path(path).expect("valid unix pathname")
}

fn test_pathname_nonblock_eof(path: &str) {
    let _ = unlink(path);
    let (addr, addrlen) = unix_addr(path);
    let listener = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0);
    assert!(listener >= 0);
    assert_eq!(bind_unix(listener as usize, &addr, addrlen), 0);
    assert_eq!(listen(listener as usize, 4), 0);
    assert_eq!(accept_unix(listener as usize), -EAGAIN);

    let client = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(client >= 0);
    assert_eq!(connect_unix(client as usize, &addr, addrlen), 0);
    let server = accept_unix(listener as usize);
    assert!(server >= 0);

    let payload = b"pathname\0";
    let mut received = [0u8; 9];
    assert_eq!(write(client as usize, payload), payload.len() as isize);
    assert_eq!(read(server as usize, &mut received), payload.len() as isize);
    assert_eq!(&received, payload);
    assert_eq!(close(client as usize), 0);
    assert_eq!(read(server as usize, &mut received), 0);
    assert_eq!(write(server as usize, payload), -EPIPE);
    assert_eq!(close(server as usize), 0);
    assert_eq!(close(listener as usize), 0);

    let stale_client = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(stale_client >= 0);
    assert_eq!(
        connect_unix(stale_client as usize, &addr, addrlen),
        -ECONNREFUSED
    );
    assert_eq!(close(stale_client as usize), 0);
    println!("SOCKET_PHASE5 pathname_nonblock_eof PASS");
}

fn test_shutdown_and_poll() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(write(fds[0] as usize, b"b"), 1);
    assert_eq!(shutdown(fds[0] as usize, SHUT_WR), 0);

    let mut pollfds = [PollFd {
        fd: fds[1],
        events: POLLIN | POLLOUT | POLLRDHUP,
        revents: 0,
    }];
    let timeout = user_lib::TimeSpec { sec: 0, nsec: 0 };
    assert_eq!(ppoll_raw(&mut pollfds, &timeout, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLIN, 0);
    assert_ne!(pollfds[0].revents & POLLOUT, 0);
    assert_ne!(pollfds[0].revents & POLLRDHUP, 0);
    assert_eq!(pollfds[0].revents & POLLHUP, 0);

    let epfd = epoll_create1(0);
    assert!(epfd >= 0);
    let epfd = epfd as usize;
    let mut interest = [0u8; 12];
    interest[..4].copy_from_slice(&((POLLIN | POLLRDHUP) as u32).to_ne_bytes());
    interest[4..].copy_from_slice(&0x5244485550u64.to_ne_bytes());
    assert_eq!(
        epoll_ctl(epfd, EPOLL_CTL_ADD, fds[1] as usize, interest.as_ptr()),
        0
    );
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

    let mut byte = [0u8; 1];
    assert_eq!(read(fds[1] as usize, &mut byte), 1);
    assert_eq!(byte[0], b'b');
    assert_eq!(read(fds[1] as usize, &mut byte), 0);
    assert_eq!(write(fds[0] as usize, b"x"), -EPIPE);
    assert_eq!(write(fds[1] as usize, b"y"), 1);
    assert_eq!(read(fds[0] as usize, &mut byte), 1);
    assert_eq!(byte[0], b'y');

    assert_eq!(close(fds[0] as usize), 0);
    pollfds[0].revents = 0;
    assert_eq!(ppoll_raw(&mut pollfds, &timeout, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLIN, 0);
    assert_ne!(pollfds[0].revents & POLLHUP, 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("SOCKET_PHASE5 shutdown_poll_rdhup PASS");
}

fn test_blocking_poll_and_pipe_events() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(fds[0] as usize), 0);
        delay_ms(50);
        assert_eq!(write(fds[1] as usize, b"w"), 1);
        exit(0);
        unreachable!();
    }
    assert_eq!(close(fds[1] as usize), 0);
    let mut pollfds = [PollFd {
        fd: fds[0],
        events: POLLIN,
        revents: 0,
    }];
    let timeout = user_lib::TimeSpec { sec: 1, nsec: 0 };
    assert_eq!(ppoll_raw(&mut pollfds, &timeout, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLIN, 0);
    let mut byte = [0u8; 1];
    assert_eq!(read(fds[0] as usize, &mut byte), 1);
    assert_eq!(byte[0], b'w');
    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);

    let zero = user_lib::TimeSpec { sec: 0, nsec: 0 };
    let mut pipefds = [-1i32; 2];
    assert_eq!(pipe(&mut pipefds), 0);
    pollfds[0] = PollFd {
        fd: pipefds[0],
        events: 0,
        revents: 0,
    };
    assert_eq!(ppoll_raw(&mut pollfds, &zero, core::ptr::null(), 0), 0);
    assert_eq!(close(pipefds[1] as usize), 0);
    assert_eq!(ppoll_raw(&mut pollfds, &zero, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLHUP, 0);
    assert_eq!(close(pipefds[0] as usize), 0);

    assert_eq!(pipe(&mut pipefds), 0);
    pollfds[0] = PollFd {
        fd: pipefds[1],
        events: 0,
        revents: 0,
    };
    assert_eq!(close(pipefds[0] as usize), 0);
    assert_eq!(ppoll_raw(&mut pollfds, &zero, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLERR, 0);
    assert_eq!(close(pipefds[1] as usize), 0);

    assert_eq!(pipe(&mut pipefds), 0);
    let epfd = epoll_create1(0);
    assert!(epfd >= 0);
    let mut interest = [0u8; 12];
    interest[4..].copy_from_slice(&0x12345678u64.to_ne_bytes());
    assert_eq!(
        epoll_ctl(
            epfd as usize,
            EPOLL_CTL_ADD,
            pipefds[0] as usize,
            interest.as_ptr()
        ),
        0
    );
    assert_eq!(close(pipefds[1] as usize), 0);
    let mut ready = [0u8; 12];
    assert_eq!(
        epoll_pwait(
            epfd as usize,
            ready.as_mut_ptr(),
            1,
            0,
            core::ptr::null(),
            0
        ),
        1
    );
    let events = u32::from_ne_bytes(ready[..4].try_into().unwrap());
    let data = u64::from_ne_bytes(ready[4..].try_into().unwrap());
    assert_ne!(events & POLLHUP as u32, 0);
    assert_eq!(data, 0x12345678);
    assert_eq!(close(pipefds[0] as usize), 0);
    assert_eq!(close(epfd as usize), 0);
    println!("SOCKET_PHASE5 blocking_poll_pipe_events PASS");
}

fn test_accept_eintr(path: &str) {
    let _ = unlink(path);
    let (addr, addrlen) = unix_addr(path);
    let listener = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(listener >= 0);
    assert_eq!(bind_unix(listener as usize, &addr, addrlen), 0);
    assert_eq!(listen(listener as usize, 4), 0);

    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        exit(0);
        unreachable!();
    }

    assert_eq!(accept_unix(listener as usize), -EINTR);
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), 1);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(listener as usize), 0);
    println!("SOCKET_PHASE5 accept_eintr PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let ignore_pipe = SignalAction {
        handler: 1,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGPIPE, Some(&ignore_pipe), None), 0);
    let interrupt = SignalAction {
        handler: signal_handler as usize,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&interrupt), None), 0);

    let path = "/tmp/respos_socket_phase5.sock\0";
    let eintr_path = "/tmp/respos_socket_eintr.sock\0";
    test_pathname_nonblock_eof(path);
    test_shutdown_and_poll();
    test_blocking_poll_and_pipe_events();
    test_accept_eintr(eintr_path);
    assert_eq!(unlink(path), 0);
    assert_eq!(unlink(eintr_path), 0);
    println!("SOCKET_PHASE5 ALL PASS");
    0
}
