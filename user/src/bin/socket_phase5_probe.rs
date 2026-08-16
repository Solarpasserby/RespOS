#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    AF_UNIX, IoVec, MMsgHdr, MsgHdr, PollFd, SIGPIPE, SIGUSR1, SIGWINCH, SOCK_NONBLOCK,
    SOCK_STREAM, SignalAction, SockAddrUn, TimeSpec, accept_unix, bind_unix, close, connect_unix,
    epoll_create1, epoll_ctl, epoll_pwait, exit, fork, getpid, kill, listen, pipe, ppoll_raw, read,
    recvfrom, recvmmsg, recvmmsg_with_timeout, recvmsg, sendmmsg, sendmsg, sendto, setsockopt_raw,
    shutdown, sigaction, socket, socketpair, time_get, unlink, waitpid, write, yield_,
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
const EPOLL_CTL_MOD: usize = 3;
const EPOLLET: u32 = 1 << 31;
const EPOLLONESHOT: u32 = 1 << 30;
const SA_RESTART: u32 = 0x1000_0000;
const MSG_DONTWAIT: usize = 0x40;
const MSG_WAITALL: usize = 0x100;
const MSG_WAITFORONE: usize = 0x1_0000;
const SOL_SOCKET: usize = 1;
const SO_RCVTIMEO: usize = 20;
const SO_SNDTIMEO: usize = 21;
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

fn epoll_event(events: u32, data: u64) -> [u8; 12] {
    let mut event = [0u8; 12];
    event[..4].copy_from_slice(&events.to_ne_bytes());
    event[4..].copy_from_slice(&data.to_ne_bytes());
    event
}

fn test_rdhup_blocking_and_epoll_modes() {
    let mut fds = [-1i32; 2];
    let mut ack = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(pipe(&mut ack), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(fds[1] as usize), 0);
        assert_eq!(close(ack[1] as usize), 0);
        delay_ms(50);
        assert_eq!(write(fds[0] as usize, b"p"), 1);
        assert_eq!(shutdown(fds[0] as usize, SHUT_WR), 0);
        let mut byte = [0u8; 1];
        assert_eq!(read(ack[0] as usize, &mut byte), 1);
        exit(0);
        unreachable!();
    }
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(ack[0] as usize), 0);
    let mut pollfds = [PollFd {
        fd: fds[1],
        events: POLLRDHUP,
        revents: 0,
    }];
    let timeout = user_lib::TimeSpec { sec: 1, nsec: 0 };
    assert_eq!(ppoll_raw(&mut pollfds, &timeout, core::ptr::null(), 0), 1);
    assert_ne!(pollfds[0].revents & POLLRDHUP, 0);
    assert_eq!(pollfds[0].revents & (POLLIN | POLLHUP), 0);
    let mut byte = [0u8; 1];
    assert_eq!(read(fds[1] as usize, &mut byte), 1);
    assert_eq!(byte[0], b'p');
    assert_eq!(write(ack[1] as usize, b"a"), 1);
    assert_eq!(close(ack[1] as usize), 0);
    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[1] as usize), 0);

    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(pipe(&mut ack), 0);
    let epfd = epoll_create1(0);
    assert!(epfd >= 0);
    let epfd = epfd as usize;
    let mut interest = epoll_event(POLLRDHUP as u32, 0x1001);
    assert_eq!(
        epoll_ctl(epfd, EPOLL_CTL_ADD, fds[1] as usize, interest.as_ptr()),
        0
    );
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(fds[1] as usize), 0);
        assert_eq!(close(epfd), 0);
        assert_eq!(close(ack[1] as usize), 0);
        delay_ms(50);
        assert_eq!(write(fds[0] as usize, b"e"), 1);
        assert_eq!(shutdown(fds[0] as usize, SHUT_WR), 0);
        let mut child_byte = [0u8; 1];
        assert_eq!(read(ack[0] as usize, &mut child_byte), 1);
        exit(0);
        unreachable!();
    }
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(ack[0] as usize), 0);
    let mut ready = [0u8; 12];
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 1000, core::ptr::null(), 0),
        1
    );
    assert_eq!(
        u32::from_ne_bytes(ready[..4].try_into().unwrap()),
        POLLRDHUP as u32
    );
    assert_eq!(u64::from_ne_bytes(ready[4..].try_into().unwrap()), 0x1001);

    interest = epoll_event(POLLRDHUP as u32 | EPOLLET, 0x2002);
    assert_eq!(
        epoll_ctl(epfd, EPOLL_CTL_MOD, fds[1] as usize, interest.as_ptr()),
        0
    );
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 0, core::ptr::null(), 0),
        1
    );
    assert_eq!(
        u32::from_ne_bytes(ready[..4].try_into().unwrap()),
        POLLRDHUP as u32
    );
    assert_eq!(u64::from_ne_bytes(ready[4..].try_into().unwrap()), 0x2002);
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 0, core::ptr::null(), 0),
        0
    );

    interest = epoll_event(POLLRDHUP as u32 | EPOLLONESHOT, 0x3003);
    assert_eq!(
        epoll_ctl(epfd, EPOLL_CTL_MOD, fds[1] as usize, interest.as_ptr()),
        0
    );
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 0, core::ptr::null(), 0),
        1
    );
    assert_eq!(
        u32::from_ne_bytes(ready[..4].try_into().unwrap()),
        POLLRDHUP as u32
    );
    assert_eq!(u64::from_ne_bytes(ready[4..].try_into().unwrap()), 0x3003);
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 0, core::ptr::null(), 0),
        0
    );
    assert_eq!(
        epoll_ctl(epfd, EPOLL_CTL_MOD, fds[1] as usize, interest.as_ptr()),
        0
    );
    assert_eq!(
        epoll_pwait(epfd, ready.as_mut_ptr(), 1, 0, core::ptr::null(), 0),
        1
    );
    assert_eq!(
        u32::from_ne_bytes(ready[..4].try_into().unwrap()),
        POLLRDHUP as u32
    );

    assert_eq!(read(fds[1] as usize, &mut byte), 1);
    assert_eq!(byte[0], b'e');
    assert_eq!(write(ack[1] as usize, b"a"), 1);
    assert_eq!(close(ack[1] as usize), 0);
    status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[1] as usize), 0);
    assert_eq!(close(epfd), 0);
    println!("SOCKET_PHASE5 rdhup_blocking_edge_oneshot PASS");
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

fn test_accept_signal(
    path: &str,
    signo: i32,
    recv_timeout: bool,
    expect_eintr: bool,
    expect_handler: bool,
) {
    let _ = unlink(path);
    let (addr, addrlen) = unix_addr(path);
    let listener = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(listener >= 0);
    assert_eq!(bind_unix(listener as usize, &addr, addrlen), 0);
    assert_eq!(listen(listener as usize, 4), 0);
    if recv_timeout {
        let timeout = user_lib::TimeVal { sec: 2, usec: 0 };
        assert_eq!(
            setsockopt_raw(listener as usize, SOL_SOCKET, SO_RCVTIMEO, &timeout),
            0
        );
    }

    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            let client = socket(AF_UNIX, SOCK_STREAM, 0);
            assert!(client >= 0);
            assert_eq!(connect_unix(client as usize, &addr, addrlen), 0);
            assert_eq!(close(client as usize), 0);
        }
        exit(0);
        unreachable!();
    }

    let accepted = accept_unix(listener as usize);
    if expect_eintr {
        assert_eq!(accepted, -EINTR);
    } else {
        assert!(accepted >= 0);
        assert_eq!(close(accepted as usize), 0);
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(listener as usize), 0);
    assert_eq!(unlink(path), 0);
}

fn test_connect_signal(
    path: &str,
    signo: i32,
    send_timeout: bool,
    expect_eintr: bool,
    expect_handler: bool,
) {
    let _ = unlink(path);
    let (addr, addrlen) = unix_addr(path);
    let listener = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(listener >= 0);
    assert_eq!(bind_unix(listener as usize, &addr, addrlen), 0);
    assert_eq!(listen(listener as usize, 4), 0);

    let mut fillers = [-1isize; 256];
    let mut filler_count = 0usize;
    while filler_count < fillers.len() {
        let client = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0);
        assert!(client >= 0);
        let result = connect_unix(client as usize, &addr, addrlen);
        if result == 0 {
            fillers[filler_count] = client;
            filler_count += 1;
            continue;
        }
        assert_eq!(result, -EAGAIN);
        assert_eq!(close(client as usize), 0);
        break;
    }
    assert!(filler_count > 0 && filler_count < fillers.len());

    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            let accepted = accept_unix(listener as usize);
            assert!(accepted >= 0);
            assert_eq!(close(accepted as usize), 0);
        }
        exit(0);
        unreachable!();
    }

    let client = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(client >= 0);
    if send_timeout {
        let timeout = user_lib::TimeVal { sec: 2, usec: 0 };
        assert_eq!(
            setsockopt_raw(client as usize, SOL_SOCKET, SO_SNDTIMEO, &timeout),
            0
        );
    }
    let connected = connect_unix(client as usize, &addr, addrlen);
    if expect_eintr {
        assert_eq!(connected, -EINTR);
    } else {
        assert_eq!(connected, 0);
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    assert_eq!(close(client as usize), 0);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    for filler in fillers.iter().take(filler_count) {
        assert_eq!(close(*filler as usize), 0);
    }
    assert_eq!(close(listener as usize), 0);
    assert_eq!(unlink(path), 0);
}

fn test_recvfrom_signal(signo: i32, recv_timeout: bool, expect_eintr: bool, expect_handler: bool) {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    if recv_timeout {
        let timeout = user_lib::TimeVal { sec: 2, usec: 0 };
        assert_eq!(
            setsockopt_raw(fds[0] as usize, SOL_SOCKET, SO_RCVTIMEO, &timeout),
            0
        );
    }
    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            assert_eq!(sendto(fds[1] as usize, b"r", 0, None), 1);
        }
        exit(0);
        unreachable!();
    }

    let mut byte = [0u8; 1];
    let received = recvfrom(fds[0] as usize, &mut byte, 0, None);
    if expect_eintr {
        assert_eq!(received, -EINTR);
    } else {
        assert_eq!(received, 1);
        assert_eq!(byte[0], b'r');
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
}

fn test_sendto_signal(signo: i32, send_timeout: bool, expect_eintr: bool, expect_handler: bool) {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let chunk = [0u8; 4096];
    loop {
        let sent = sendto(fds[0] as usize, &chunk, MSG_DONTWAIT, None);
        if sent == -EAGAIN {
            break;
        }
        assert!(sent > 0);
    }
    if send_timeout {
        let timeout = user_lib::TimeVal { sec: 2, usec: 0 };
        assert_eq!(
            setsockopt_raw(fds[0] as usize, SOL_SOCKET, SO_SNDTIMEO, &timeout),
            0
        );
    }

    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            let mut drained = [0u8; 4096];
            let mut total = 0isize;
            loop {
                let received = recvfrom(fds[1] as usize, &mut drained, MSG_DONTWAIT, None);
                if received == -EAGAIN {
                    break;
                }
                assert!(received > 0);
                total += received;
            }
            assert!(total > 0);
        }
        exit(0);
        unreachable!();
    }

    let sent = sendto(fds[0] as usize, b"s", 0, None);
    if expect_eintr {
        assert_eq!(sent, -EINTR);
    } else {
        assert_eq!(sent, 1);
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
}

fn test_recvmsg_signal(signo: i32, expect_eintr: bool, expect_handler: bool) {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            assert_eq!(sendto(fds[1] as usize, b"mv", 0, None), 2);
        }
        exit(0);
        unreachable!();
    }

    let mut first = [0u8; 1];
    let mut second = [0u8; 1];
    let mut iovs = [
        IoVec {
            base: first.as_mut_ptr(),
            len: 1,
        },
        IoVec {
            base: second.as_mut_ptr(),
            len: 1,
        },
    ];
    let mut msg = MsgHdr {
        msg_iov: iovs.as_mut_ptr() as usize,
        msg_iovlen: iovs.len(),
        ..MsgHdr::default()
    };
    let received = recvmsg(fds[0] as usize, &mut msg, 0);
    if expect_eintr {
        assert_eq!(received, -EINTR);
    } else {
        assert_eq!(received, 2);
        assert_eq!(first[0], b'm');
        assert_eq!(second[0], b'v');
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
}

fn test_recvmsg_partial_signal() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        assert_eq!(sendto(fds[1] as usize, b"p", 0, None), 1);
        delay_ms(100);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        exit(0);
        unreachable!();
    }

    let mut bytes = [0u8; 2];
    let mut iov = IoVec {
        base: bytes.as_mut_ptr(),
        len: bytes.len(),
    };
    let mut msg = MsgHdr {
        msg_iov: (&mut iov as *mut IoVec) as usize,
        msg_iovlen: 1,
        ..MsgHdr::default()
    };
    assert_eq!(recvmsg(fds[0] as usize, &mut msg, MSG_WAITALL), 1);
    assert_eq!(bytes[0], b'p');
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), 1);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
}

fn test_sendmsg_signal(signo: i32, expect_eintr: bool, expect_handler: bool) {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let chunk = [0u8; 4096];
    loop {
        let sent = sendto(fds[0] as usize, &chunk, MSG_DONTWAIT, None);
        if sent == -EAGAIN {
            break;
        }
        assert!(sent > 0);
    }

    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            let mut drained = [0u8; 4096];
            while recvfrom(fds[1] as usize, &mut drained, MSG_DONTWAIT, None) > 0 {}
        }
        exit(0);
        unreachable!();
    }

    let first = [b'm'];
    let second = [b's'];
    let iovs = [
        IoVec {
            base: first.as_ptr() as *mut u8,
            len: 1,
        },
        IoVec {
            base: second.as_ptr() as *mut u8,
            len: 1,
        },
    ];
    let msg = MsgHdr {
        msg_iov: iovs.as_ptr() as usize,
        msg_iovlen: iovs.len(),
        ..MsgHdr::default()
    };
    let sent = sendmsg(fds[0] as usize, &msg, 0);
    if expect_eintr {
        assert_eq!(sent, -EINTR);
    } else {
        assert_eq!(sent, 2);
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
}

fn test_recvmmsg_signal(signo: i32, expect_eintr: bool, expect_handler: bool) {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            assert_eq!(sendto(fds[1] as usize, b"mm", 0, None), 2);
        }
        exit(0);
        unreachable!();
    }

    let mut bytes = [0u8; 2];
    let mut iov = IoVec {
        base: bytes.as_mut_ptr(),
        len: bytes.len(),
    };
    let mut messages = [MMsgHdr {
        msg_hdr: MsgHdr {
            msg_iov: (&mut iov as *mut IoVec) as usize,
            msg_iovlen: 1,
            ..MsgHdr::default()
        },
        msg_len: 0,
    }];
    let received = recvmmsg(fds[0] as usize, &mut messages, 0);
    if expect_eintr {
        assert_eq!(received, -EINTR);
        assert_eq!(messages[0].msg_len, 0);
    } else {
        assert_eq!(received, 1);
        assert_eq!(messages[0].msg_len, 2);
        assert_eq!(&bytes, b"mm");
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
}

fn test_recvmmsg_partial_signal() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        assert_eq!(sendto(fds[1] as usize, b"p", 0, None), 1);
        delay_ms(100);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        exit(0);
        unreachable!();
    }

    let mut first = [0u8; 1];
    let mut second = [0u8; 1];
    let mut iovs = [
        IoVec {
            base: first.as_mut_ptr(),
            len: 1,
        },
        IoVec {
            base: second.as_mut_ptr(),
            len: 1,
        },
    ];
    let mut messages = [
        MMsgHdr {
            msg_hdr: MsgHdr {
                msg_iov: (&mut iovs[0] as *mut IoVec) as usize,
                msg_iovlen: 1,
                ..MsgHdr::default()
            },
            msg_len: 0,
        },
        MMsgHdr {
            msg_hdr: MsgHdr {
                msg_iov: (&mut iovs[1] as *mut IoVec) as usize,
                msg_iovlen: 1,
                ..MsgHdr::default()
            },
            msg_len: 0,
        },
    ];
    assert_eq!(recvmmsg(fds[0] as usize, &mut messages, 0), 1);
    assert_eq!(messages[0].msg_len, 1);
    assert_eq!(messages[1].msg_len, 0);
    assert_eq!(first[0], b'p');
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), 1);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
}

fn timespec_ms(value: &TimeSpec) -> usize {
    value
        .sec
        .saturating_mul(1000)
        .saturating_add(value.nsec / 1_000_000)
}

fn test_recvmmsg_timeout_modes() {
    let parent = getpid();
    assert!(parent > 0);

    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(300);
        assert_eq!(sendto(fds[1] as usize, b"t", 0, None), 1);
        exit(0);
        unreachable!();
    }
    let mut byte = [0u8; 1];
    let mut iov = IoVec {
        base: byte.as_mut_ptr(),
        len: 1,
    };
    let mut message = [MMsgHdr {
        msg_hdr: MsgHdr {
            msg_iov: (&mut iov as *mut IoVec) as usize,
            msg_iovlen: 1,
            ..MsgHdr::default()
        },
        msg_len: 0,
    }];
    let mut timeout = TimeSpec {
        sec: 0,
        nsec: 200_000_000,
    };
    let started = time_get();
    assert_eq!(
        recvmmsg_with_timeout(fds[0] as usize, &mut message, 0, &mut timeout),
        1
    );
    let elapsed = time_get().saturating_sub(started);
    assert_eq!(message[0].msg_len, 1);
    assert_eq!(byte[0], b't');
    assert!((250..1000).contains(&elapsed));
    assert_eq!(timespec_ms(&timeout), 0);
    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);

    {
        let mut wait_fds = [-1i32; 2];
        assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut wait_fds), 0);
        assert_eq!(sendto(wait_fds[1] as usize, b"w", 0, None), 1);
        let mut wait_bytes = [0u8; 2];
        let mut wait_iovs = [
            IoVec {
                base: wait_bytes.as_mut_ptr(),
                len: 1,
            },
            IoVec {
                base: wait_bytes.as_mut_ptr().wrapping_add(1),
                len: 1,
            },
        ];
        let mut wait_messages = [
            MMsgHdr {
                msg_hdr: MsgHdr {
                    msg_iov: (&mut wait_iovs[0] as *mut IoVec) as usize,
                    msg_iovlen: 1,
                    ..MsgHdr::default()
                },
                msg_len: 0,
            },
            MMsgHdr {
                msg_hdr: MsgHdr {
                    msg_iov: (&mut wait_iovs[1] as *mut IoVec) as usize,
                    msg_iovlen: 1,
                    ..MsgHdr::default()
                },
                msg_len: 0,
            },
        ];
        let mut wait_timeout = TimeSpec {
            sec: 0,
            nsec: 400_000_000,
        };
        let wait_started = time_get();
        assert_eq!(
            recvmmsg_with_timeout(
                wait_fds[0] as usize,
                &mut wait_messages,
                MSG_WAITFORONE,
                &mut wait_timeout,
            ),
            1
        );
        assert!(time_get().saturating_sub(wait_started) < 100);
        assert_eq!(wait_messages[0].msg_len, 1);
        assert_eq!(wait_messages[1].msg_len, 0);
        assert_eq!(wait_bytes, *b"w\0");
        assert!((300..=400).contains(&timespec_ms(&wait_timeout)));
        assert_eq!(close(wait_fds[0] as usize), 0);
        assert_eq!(close(wait_fds[1] as usize), 0);
    }

    let interrupt = SignalAction {
        handler: signal_handler as usize,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&interrupt), None), 0);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    fds = [-1; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        exit(0);
        unreachable!();
    }
    byte[0] = 0;
    message[0].msg_len = 0;
    timeout = TimeSpec {
        sec: 0,
        nsec: 400_000_000,
    };
    let started = time_get();
    assert_eq!(
        recvmmsg_with_timeout(fds[0] as usize, &mut message, 0, &mut timeout),
        -EINTR
    );
    let elapsed = time_get().saturating_sub(started);
    assert!(elapsed < 300);
    assert_eq!(timespec_ms(&timeout), 400);
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), 1);
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);

    let restart = SignalAction {
        handler: signal_handler as usize,
        flags: SA_RESTART,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&restart), None), 0);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    fds = [-1; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        delay_ms(100);
        assert_eq!(sendto(fds[1] as usize, b"d", 0, None), 1);
        exit(0);
        unreachable!();
    }
    byte[0] = 0;
    message[0].msg_len = 0;
    timeout = TimeSpec {
        sec: 0,
        nsec: 400_000_000,
    };
    let started = time_get();
    assert_eq!(
        recvmmsg_with_timeout(fds[0] as usize, &mut message, 0, &mut timeout),
        1
    );
    let elapsed = time_get().saturating_sub(started);
    assert_eq!(message[0].msg_len, 1);
    assert_eq!(byte[0], b'd');
    assert!((150..350).contains(&elapsed));
    assert!((150..=400).contains(&timespec_ms(&timeout)));
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), 1);
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);

    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    fds = [-1; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    assert_eq!(sendto(fds[1] as usize, b"p", 0, None), 1);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, SIGUSR1), 0);
        exit(0);
        unreachable!();
    }
    let mut bytes = [0u8; 2];
    let mut iovs = [
        IoVec {
            base: bytes.as_mut_ptr(),
            len: 1,
        },
        IoVec {
            base: bytes.as_mut_ptr().wrapping_add(1),
            len: 1,
        },
    ];
    let mut messages = [
        MMsgHdr {
            msg_hdr: MsgHdr {
                msg_iov: (&mut iovs[0] as *mut IoVec) as usize,
                msg_iovlen: 1,
                ..MsgHdr::default()
            },
            msg_len: 0,
        },
        MMsgHdr {
            msg_hdr: MsgHdr {
                msg_iov: (&mut iovs[1] as *mut IoVec) as usize,
                msg_iovlen: 1,
                ..MsgHdr::default()
            },
            msg_len: 0,
        },
    ];
    timeout = TimeSpec {
        sec: 0,
        nsec: 400_000_000,
    };
    let started = time_get();
    assert_eq!(
        recvmmsg_with_timeout(fds[0] as usize, &mut messages, 0, &mut timeout),
        1
    );
    let elapsed = time_get().saturating_sub(started);
    assert!(elapsed < 300);
    assert_eq!(messages[0].msg_len, 1);
    assert_eq!(messages[1].msg_len, 0);
    assert_eq!(bytes, *b"p\0");
    assert!((250..=400).contains(&timespec_ms(&timeout)));
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), 1);
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);

    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    fds = [-1; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, SIGWINCH), 0);
        delay_ms(100);
        assert_eq!(sendto(fds[1] as usize, b"i", 0, None), 1);
        exit(0);
        unreachable!();
    }
    byte[0] = 0;
    message[0].msg_len = 0;
    timeout = TimeSpec {
        sec: 0,
        nsec: 400_000_000,
    };
    let started = time_get();
    assert_eq!(
        recvmmsg_with_timeout(fds[0] as usize, &mut message, 0, &mut timeout),
        1
    );
    let elapsed = time_get().saturating_sub(started);
    assert_eq!(message[0].msg_len, 1);
    assert_eq!(byte[0], b'i');
    assert!((150..350).contains(&elapsed));
    assert!((100..350).contains(&timespec_ms(&timeout)));
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), 0);
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("SOCKET_PHASE5 recvmmsg_timeout_modes PASS");
}

fn test_sendmmsg_signal(signo: i32, expect_eintr: bool, expect_handler: bool) {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let chunk = [0u8; 4096];
    loop {
        let sent = sendto(fds[0] as usize, &chunk, MSG_DONTWAIT, None);
        if sent == -EAGAIN {
            break;
        }
        assert!(sent > 0);
    }
    let parent = getpid();
    let signaler = fork();
    assert!(signaler >= 0);
    if signaler == 0 {
        delay_ms(100);
        assert_eq!(kill(parent as usize, signo), 0);
        if !expect_eintr {
            delay_ms(100);
            let mut drained = [0u8; 4096];
            while recvfrom(fds[1] as usize, &mut drained, MSG_DONTWAIT, None) > 0 {}
        }
        exit(0);
        unreachable!();
    }

    let first = [b'm'];
    let second = [b'm'];
    let mut iovs = [
        IoVec {
            base: first.as_ptr() as *mut u8,
            len: 1,
        },
        IoVec {
            base: second.as_ptr() as *mut u8,
            len: 1,
        },
    ];
    let mut messages = [MMsgHdr {
        msg_hdr: MsgHdr {
            msg_iov: iovs.as_mut_ptr() as usize,
            msg_iovlen: iovs.len(),
            ..MsgHdr::default()
        },
        msg_len: 0,
    }];
    let sent = sendmmsg(fds[0] as usize, &mut messages, 0);
    if expect_eintr {
        assert_eq!(sent, -EINTR);
        assert_eq!(messages[0].msg_len, 0);
    } else {
        assert_eq!(sent, 1);
        assert_eq!(messages[0].msg_len, 2);
    }
    assert_eq!(SIGNAL_SEEN.load(Ordering::SeqCst), expect_handler as usize);
    let mut status = -1;
    assert_eq!(waitpid(signaler as usize, &mut status), signaler);
    assert_eq!(status, 0);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
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
    test_pathname_nonblock_eof(path);
    test_shutdown_and_poll();
    test_rdhup_blocking_and_epoll_modes();
    test_blocking_poll_and_pipe_events();
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_accept_signal(
        "/tmp/respos_socket_eintr.sock\0",
        SIGUSR1,
        false,
        true,
        true,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_connect_signal(
        "/tmp/respos_connect_eintr.sock\0",
        SIGUSR1,
        false,
        true,
        true,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvfrom_signal(SIGUSR1, false, true, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendto_signal(SIGUSR1, false, true, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmsg_signal(SIGUSR1, true, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendmsg_signal(SIGUSR1, true, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmmsg_signal(SIGUSR1, true, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendmmsg_signal(SIGUSR1, true, true);
    let restart = SignalAction {
        handler: signal_handler as usize,
        flags: SA_RESTART,
        ..SignalAction::default()
    };
    assert_eq!(sigaction(SIGUSR1, Some(&restart), None), 0);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_accept_signal(
        "/tmp/respos_socket_restart.sock\0",
        SIGUSR1,
        false,
        false,
        true,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_accept_signal(
        "/tmp/respos_accept_timeout.sock\0",
        SIGUSR1,
        true,
        true,
        true,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_connect_signal(
        "/tmp/respos_connect_restart.sock\0",
        SIGUSR1,
        false,
        false,
        true,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_connect_signal(
        "/tmp/respos_connect_timeout.sock\0",
        SIGUSR1,
        true,
        true,
        true,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvfrom_signal(SIGUSR1, false, false, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendto_signal(SIGUSR1, false, false, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvfrom_signal(SIGUSR1, true, true, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendto_signal(SIGUSR1, true, true, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmsg_signal(SIGUSR1, false, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendmsg_signal(SIGUSR1, false, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmsg_partial_signal();
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmmsg_signal(SIGUSR1, false, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendmmsg_signal(SIGUSR1, false, true);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmmsg_partial_signal();
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_accept_signal(
        "/tmp/respos_socket_ignored.sock\0",
        SIGWINCH,
        false,
        false,
        false,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_connect_signal(
        "/tmp/respos_connect_ignored.sock\0",
        SIGWINCH,
        false,
        false,
        false,
    );
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvfrom_signal(SIGWINCH, false, false, false);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendto_signal(SIGWINCH, false, false, false);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmsg_signal(SIGWINCH, false, false);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendmsg_signal(SIGWINCH, false, false);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_recvmmsg_signal(SIGWINCH, false, false);
    SIGNAL_SEEN.store(0, Ordering::SeqCst);
    test_sendmmsg_signal(SIGWINCH, false, false);
    test_recvmmsg_timeout_modes();
    println!("SOCKET_PHASE5 connect_restart_modes PASS");
    println!("SOCKET_PHASE5 restart_modes PASS");
    assert_eq!(unlink(path), 0);
    println!("SOCKET_PHASE5 ALL PASS");
    0
}
