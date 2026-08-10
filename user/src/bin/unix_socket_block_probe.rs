#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{AF_UNIX, SOCK_STREAM, close, exit, fork, read, socketpair, waitpid, write};

const PAYLOAD_SIZE: usize = 128 * 1024;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);

    let pid = fork();
    assert!(pid >= 0);
    if pid == 0 {
        close(fds[0] as usize);
        let mut received = 0usize;
        let mut buf = [0u8; 4096];
        while received < PAYLOAD_SIZE {
            let n = read(fds[1] as usize, &mut buf);
            assert!(n > 0);
            assert!(
                buf[..n as usize]
                    .iter()
                    .enumerate()
                    .all(|(offset, byte)| *byte == ((received + offset) / 4096) as u8)
            );
            received += n as usize;
        }
        close(fds[1] as usize);
        exit(0);
    }

    close(fds[1] as usize);
    let page = [0u8; 4096];
    for index in 0..PAYLOAD_SIZE / page.len() {
        let mut data = page;
        data.fill(index as u8);
        let mut written = 0usize;
        while written < data.len() {
            let n = write(fds[0] as usize, &data[written..]);
            assert!(n > 0);
            written += n as usize;
        }
    }
    close(fds[0] as usize);
    let mut status = -1;
    assert_eq!(waitpid(pid as usize, &mut status), pid);
    assert_eq!(status, 0);
    println!("UNIX_SOCKET_BLOCK_PROBE_PASS bytes={}", PAYLOAD_SIZE);
    0
}
