#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_INET, AF_UNIX, SOCK_STREAM, close, pipe, read, socket, socketpair, splice_raw, write,
};

const EBADF: isize = 9;
const EINVAL: isize = 22;
const ENOTCONN: isize = 107;

fn test_unconnected_socket_errors() {
    let mut pipefd = [-1i32; 2];
    assert_eq!(pipe(&mut pipefd), 0);
    let unix_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(unix_fd >= 0);
    let inet_fd = socket(AF_INET, SOCK_STREAM, 0);
    assert!(inet_fd >= 0);

    assert_eq!(
        splice_raw(unix_fd as usize, 0, pipefd[1] as usize, 0, 1, 0,),
        -EINVAL
    );
    assert_eq!(
        splice_raw(inet_fd as usize, 0, pipefd[1] as usize, 0, 1, 0,),
        -ENOTCONN
    );
    assert_eq!(
        splice_raw(unix_fd as usize, 0, pipefd[0] as usize, 0, 1, 0,),
        -EBADF
    );

    assert_eq!(close(unix_fd as usize), 0);
    assert_eq!(close(inet_fd as usize), 0);
    assert_eq!(close(pipefd[0] as usize), 0);
    assert_eq!(close(pipefd[1] as usize), 0);
    println!("SPLICE_SOCKET unconnected_errors PASS");
}

fn test_connected_unix_socket_to_pipe() {
    let mut pipefd = [-1i32; 2];
    let mut sockets = [-1i32; 2];
    assert_eq!(pipe(&mut pipefd), 0);
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut sockets), 0);
    assert_eq!(write(sockets[1] as usize, b"x"), 1);
    assert_eq!(
        splice_raw(sockets[0] as usize, 0, pipefd[1] as usize, 0, 1, 0,),
        1
    );
    let mut byte = [0u8; 1];
    assert_eq!(read(pipefd[0] as usize, &mut byte), 1);
    assert_eq!(byte[0], b'x');

    assert_eq!(close(sockets[0] as usize), 0);
    assert_eq!(close(sockets[1] as usize), 0);
    assert_eq!(close(pipefd[0] as usize), 0);
    assert_eq!(close(pipefd[1] as usize), 0);
    println!("SPLICE_SOCKET connected_transfer PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_unconnected_socket_errors();
    test_connected_unix_socket_to_pipe();
    println!("SPLICE_SOCKET ALL PASS");
    0
}
