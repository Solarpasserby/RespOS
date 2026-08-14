#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_INET, AF_UNIX, O_WRONLY, SOCK_STREAM, SockAddrIn, bind, close, getpeername_raw, open,
    socket, socketpair,
};

const EBADF: isize = 9;
const EFAULT: isize = 14;
const EINVAL: isize = 22;
const ENOTSOCK: isize = 88;
const ENOTCONN: isize = 107;

fn test_descriptor_and_connection_errors() {
    let mut addr = SockAddrIn::default();
    let mut addrlen = core::mem::size_of::<SockAddrIn>() as u32;

    assert_eq!(
        getpeername_raw(
            usize::MAX,
            &mut addr as *mut SockAddrIn as usize,
            &mut addrlen as *mut u32 as usize,
        ),
        -EBADF
    );

    let file = open("/dev/null\0", O_WRONLY, 0);
    assert!(file >= 0);
    assert_eq!(
        getpeername_raw(
            file as usize,
            &mut addr as *mut SockAddrIn as usize,
            &mut addrlen as *mut u32 as usize,
        ),
        -ENOTSOCK
    );
    assert_eq!(close(file as usize), 0);

    let unconnected = socket(AF_INET, SOCK_STREAM, 0);
    assert!(unconnected >= 0);
    assert_eq!(bind(unconnected as usize, &SockAddrIn::loopback(0)), 0);
    addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(
        getpeername_raw(
            unconnected as usize,
            &mut addr as *mut SockAddrIn as usize,
            &mut addrlen as *mut u32 as usize,
        ),
        -ENOTCONN
    );
    addrlen = u32::MAX;
    assert_eq!(
        getpeername_raw(
            unconnected as usize,
            &mut addr as *mut SockAddrIn as usize,
            &mut addrlen as *mut u32 as usize,
        ),
        -ENOTCONN
    );
    assert_eq!(close(unconnected as usize), 0);
    println!("GETPEERNAME descriptor_connection PASS");
}

fn test_output_validation_precedes_peer_lookup() {
    let mut fds = [-1i32; 2];
    assert_eq!(socketpair(AF_UNIX, SOCK_STREAM, 0, &mut fds), 0);
    let mut addr = SockAddrIn::default();

    let mut addrlen = u32::MAX;
    assert_eq!(
        getpeername_raw(
            fds[0] as usize,
            &mut addr as *mut SockAddrIn as usize,
            &mut addrlen as *mut u32 as usize,
        ),
        -EINVAL
    );

    addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(
        getpeername_raw(
            fds[0] as usize,
            usize::MAX,
            &mut addrlen as *mut u32 as usize,
        ),
        -EFAULT
    );
    assert_eq!(
        getpeername_raw(fds[0] as usize, &mut addr as *mut SockAddrIn as usize, 0),
        -EFAULT
    );
    assert_eq!(
        getpeername_raw(fds[0] as usize, &mut addr as *mut SockAddrIn as usize, 1),
        -EFAULT
    );

    addr = SockAddrIn::default();
    addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(
        getpeername_raw(
            fds[0] as usize,
            &mut addr as *mut SockAddrIn as usize,
            &mut addrlen as *mut u32 as usize,
        ),
        0
    );
    assert_eq!(addr.sin_family, AF_UNIX as u16);
    assert_eq!(addrlen as usize, core::mem::size_of::<u16>());

    assert_eq!(close(fds[0] as usize), 0);
    assert_eq!(close(fds[1] as usize), 0);
    println!("GETPEERNAME output_validation PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_descriptor_and_connection_errors();
    test_output_validation_precedes_peer_lookup();
    println!("GETPEERNAME ALL PASS");
    0
}
