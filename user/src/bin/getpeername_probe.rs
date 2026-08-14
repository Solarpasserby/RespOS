#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AF_INET, AF_UNIX, O_WRONLY, SOCK_STREAM, SockAddrIn, SockAddrUn, accept_unix_addr, bind,
    bind_unix, close, connect_unix, getpeername_raw, getsockname_raw, listen, open, socket,
    socketpair, unlink,
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

    addr = SockAddrIn::default();
    addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    assert_eq!(
        getsockname_raw(
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

fn abstract_addr(name: &[u8]) -> (SockAddrUn, usize) {
    assert!(name.len() + 1 <= 108);
    let mut addr = SockAddrUn {
        sun_family: AF_UNIX as u16,
        sun_path: [0; 108],
    };
    addr.sun_path[1..1 + name.len()].copy_from_slice(name);
    (addr, core::mem::size_of::<u16>() + 1 + name.len())
}

fn check_unix_addr(
    actual: &SockAddrUn,
    actual_len: u32,
    expected: &SockAddrUn,
    expected_len: usize,
) {
    assert_eq!(actual_len as usize, expected_len);
    let actual_bytes = unsafe {
        core::slice::from_raw_parts(actual as *const SockAddrUn as *const u8, expected_len)
    };
    let expected_bytes = unsafe {
        core::slice::from_raw_parts(expected as *const SockAddrUn as *const u8, expected_len)
    };
    assert_eq!(actual_bytes, expected_bytes);
}

fn test_named_unix_addresses(abstract_namespace: bool) {
    const SERVER_PATH: &str = "/respos-getpeer-server\0";
    const CLIENT_PATH: &str = "/respos-getpeer-client\0";

    let (server_addr, server_len) = if abstract_namespace {
        abstract_addr(b"srv\xfex")
    } else {
        SockAddrUn::from_path(SERVER_PATH).unwrap()
    };
    let (client_addr, client_len) = if abstract_namespace {
        abstract_addr(b"cli\xfdy")
    } else {
        SockAddrUn::from_path(CLIENT_PATH).unwrap()
    };

    if !abstract_namespace {
        let _ = unlink(SERVER_PATH);
        let _ = unlink(CLIENT_PATH);
    }
    let listener = socket(AF_UNIX, SOCK_STREAM, 0);
    let client = socket(AF_UNIX, SOCK_STREAM, 0);
    assert!(listener >= 0 && client >= 0);
    assert_eq!(bind_unix(listener as usize, &server_addr, server_len), 0);
    assert_eq!(bind_unix(client as usize, &client_addr, client_len), 0);
    assert_eq!(listen(listener as usize, 1), 0);
    assert_eq!(connect_unix(client as usize, &server_addr, server_len), 0);

    let mut observed = SockAddrUn {
        sun_family: 0,
        sun_path: [0; 108],
    };
    let mut observed_len = core::mem::size_of::<SockAddrUn>() as u32;
    let accepted = accept_unix_addr(listener as usize, &mut observed, &mut observed_len);
    assert!(accepted >= 0);
    check_unix_addr(&observed, observed_len, &client_addr, client_len);

    observed_len = core::mem::size_of::<SockAddrUn>() as u32;
    assert_eq!(
        getsockname_raw(
            listener as usize,
            &mut observed as *mut SockAddrUn as usize,
            &mut observed_len as *mut u32 as usize,
        ),
        0
    );
    check_unix_addr(&observed, observed_len, &server_addr, server_len);

    observed_len = core::mem::size_of::<SockAddrUn>() as u32;
    assert_eq!(
        getsockname_raw(
            client as usize,
            &mut observed as *mut SockAddrUn as usize,
            &mut observed_len as *mut u32 as usize,
        ),
        0
    );
    check_unix_addr(&observed, observed_len, &client_addr, client_len);

    observed_len = core::mem::size_of::<SockAddrUn>() as u32;
    assert_eq!(
        getpeername_raw(
            client as usize,
            &mut observed as *mut SockAddrUn as usize,
            &mut observed_len as *mut u32 as usize,
        ),
        0
    );
    check_unix_addr(&observed, observed_len, &server_addr, server_len);

    observed_len = core::mem::size_of::<SockAddrUn>() as u32;
    assert_eq!(
        getsockname_raw(
            accepted as usize,
            &mut observed as *mut SockAddrUn as usize,
            &mut observed_len as *mut u32 as usize,
        ),
        0
    );
    check_unix_addr(&observed, observed_len, &server_addr, server_len);

    observed_len = core::mem::size_of::<SockAddrUn>() as u32;
    assert_eq!(
        getpeername_raw(
            accepted as usize,
            &mut observed as *mut SockAddrUn as usize,
            &mut observed_len as *mut u32 as usize,
        ),
        0
    );
    check_unix_addr(&observed, observed_len, &client_addr, client_len);

    let mut truncated = [0xaau8; 4];
    observed_len = truncated.len() as u32;
    assert_eq!(
        getpeername_raw(
            client as usize,
            truncated.as_mut_ptr() as usize,
            &mut observed_len as *mut u32 as usize,
        ),
        0
    );
    assert_eq!(observed_len as usize, server_len);
    let server_bytes = unsafe {
        core::slice::from_raw_parts(
            &server_addr as *const SockAddrUn as *const u8,
            truncated.len(),
        )
    };
    assert_eq!(truncated.as_slice(), server_bytes);

    assert_eq!(close(accepted as usize), 0);
    assert_eq!(close(client as usize), 0);
    assert_eq!(close(listener as usize), 0);
    if !abstract_namespace {
        assert_eq!(unlink(SERVER_PATH), 0);
        assert_eq!(unlink(CLIENT_PATH), 0);
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    test_descriptor_and_connection_errors();
    test_output_validation_precedes_peer_lookup();
    test_named_unix_addresses(false);
    test_named_unix_addresses(true);
    println!("GETPEERNAME ALL PASS");
    0
}
