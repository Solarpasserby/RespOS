#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_UNIX, SOCK_STREAM, SockAddrUn, accept_unix, bind_unix, close, connect_unix, exit, fork,
    getgid, getpid, getsockopt_raw, getuid, listen, pipe, read, socket, socketpair, unlink,
    waitpid, write,
};

const SOL_SOCKET: usize = 1;
const SO_PEERCRED: usize = 17;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UCred {
    pid: i32,
    uid: u32,
    gid: u32,
}

fn read_peercred(fd: usize) -> UCred {
    let mut cred = UCred::default();
    let mut len = core::mem::size_of::<UCred>() as u32;
    assert_eq!(
        getsockopt_raw(fd, SOL_SOCKET, SO_PEERCRED, &mut cred, &mut len),
        0
    );
    assert_eq!(len as usize, core::mem::size_of::<UCred>());
    cred
}

fn current_cred() -> UCred {
    UCred {
        pid: getpid() as i32,
        uid: getuid() as u32,
        gid: getgid() as u32,
    }
}

fn test_socketpair_credentials() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let expected = current_cred();
    assert_eq!(read_peercred(fds[0] as usize), expected);
    assert_eq!(read_peercred(fds[1] as usize), expected);
    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("SOCKET_PEERCRED socketpair PASS");
}

fn test_accepted_peer_snapshot(path: &str) {
    let _ = unlink(path);
    let (addr, addrlen) = SockAddrUn::from_path(path).expect("valid unix pathname");
    let listener = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(listener >= 0);
    assert_eq!(bind_unix(listener as usize, &addr, addrlen), 0);
    assert_eq!(listen(listener as usize, 4), 0);
    let listener_cred = current_cred();

    let mut release_pipe = [-1i32; 2];
    assert_eq!(pipe(&mut release_pipe), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        assert_eq!(close(release_pipe[1] as usize), 0);
        assert_eq!(close(listener as usize), 0);
        let client = socket(AF_UNIX, SOCK_STREAM, 0);
        assert!(client >= 0);
        assert_eq!(connect_unix(client as usize, &addr, addrlen), 0);
        assert_eq!(read_peercred(client as usize), listener_cred);
        let mut byte = [0u8; 1];
        assert_eq!(read(release_pipe[0] as usize, &mut byte), 1);
        assert_eq!(close(client as usize), 0);
        assert_eq!(close(release_pipe[0] as usize), 0);
        exit(0);
        unreachable!();
    }

    assert_eq!(close(release_pipe[0] as usize), 0);
    let accepted = accept_unix(listener as usize);
    assert!(accepted >= 0);
    let cred = read_peercred(accepted as usize);
    assert_eq!(cred.pid, child as i32);
    assert_eq!(cred.uid, getuid() as u32);
    assert_eq!(cred.gid, getgid() as u32);
    assert_eq!(write(release_pipe[1] as usize, b"x"), 1);
    assert_eq!(close(release_pipe[1] as usize), 0);
    assert_eq!(close(accepted as usize), 0);
    assert_eq!(close(listener as usize), 0);
    let mut status = -1;
    assert_eq!(waitpid(child as usize, &mut status), child);
    assert_eq!(status, 0);
    let _ = unlink(path);
    println!("SOCKET_PEERCRED accepted_peer PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_socketpair_credentials();
    test_accepted_peer_snapshot("socket-peercred-phase5.sock");
    println!("SOCKET_PEERCRED ALL PASS");
    0
}
