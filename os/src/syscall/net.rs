use alloc::{string::String, sync::Arc, vec, vec::Vec};
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use crate::{
    fs::{FdEntry, FileOp, OpenFlags, filename_create, vfs::InodeType},
    mm::{check_user_readable, check_user_writable, copy_from_user, copy_to_user},
    net::socket::{self, SOCK_CLOEXEC, SOCK_NONBLOCK, Socket, SocketDomain, SocketKind},
    task::current_task,
};

use super::{Errno, SysResult};

const AF_INET: u16 = 2;
const AF_UNIX: u16 = 1;
const AF_INET6: u16 = 10;
const AT_FDCWD: isize = -100;
const SOCKADDR_UN_PATH_LEN: usize = 108;
const SOCK_TYPE_MASK: usize = 0xf;
const SOL_SOCKET: usize = 1;
const SO_REUSEADDR: usize = 2;
const SO_TYPE: usize = 3;
const SO_ERROR: usize = 4;
const SO_DONTROUTE: usize = 5;
const SO_BROADCAST: usize = 6;
const SO_SNDBUF: usize = 7;
const SO_RCVBUF: usize = 8;
const SO_KEEPALIVE: usize = 9;
const SO_OOBINLINE: usize = 10;
const SO_LINGER: usize = 13;
const SO_REUSEPORT: usize = 15;
const SO_RCVTIMEO: usize = 20;
const SO_SNDTIMEO: usize = 21;
const SO_SNDBUFFORCE: usize = 32;
const SO_RCVBUFFORCE: usize = 33;
const IPPROTO_IP: usize = 0;
const IPPROTO_IPV6: usize = 41;
const IPPROTO_TCP: usize = 6;
const IPPROTO_UDP: usize = 17;
const IP_TTL: usize = 2;
const IP_RECVERR: usize = 11;
const IPV6_V6ONLY: usize = 26;
const IPPROTO_SCTP: usize = 132;
const TCP_NODELAY: usize = 1;
const TCP_MAXSEG: usize = 2;
const TCP_INFO: usize = 11;
const TCP_DEFAULT_MAXSEG: i32 = 1448;
const MSG_OOB: usize = 0x1;
const MSG_ERRQUEUE: usize = 0x2000;

const IOV_MAX: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy)]
struct MsgIov {
    base: *mut u8,
    len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MsgHdr {
    msg_name: usize,
    msg_namelen: u32,
    _pad1: u32,
    msg_iov: usize,
    msg_iovlen: usize,
    msg_control: usize,
    msg_controllen: usize,
    msg_flags: i32,
    _pad2: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MMsgHdr {
    msg_hdr: MsgHdr,
    msg_len: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn6 {
    sin6_family: u16,
    sin6_port: u16,
    sin6_flowinfo: u32,
    sin6_addr: [u8; 16],
    sin6_scope_id: u32,
}

struct SockAddrUn {
    key: String,
    pathname: Option<String>,
}

impl SockAddrIn {
    fn from_socket_addr(addr: &SocketAddr) -> Self {
        let (ip, port) = match addr {
            SocketAddr::V4(v4) => (v4.ip().octets(), v4.port()),
            SocketAddr::V6(_) => ([0, 0, 0, 0], addr.port()),
        };
        Self {
            sin_family: AF_INET,
            sin_port: port.to_be(),
            sin_addr: ip,
            sin_zero: [0; 8],
        }
    }

    fn to_socket_addr(self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(
                self.sin_addr[0],
                self.sin_addr[1],
                self.sin_addr[2],
                self.sin_addr[3],
            ),
            u16::from_be(self.sin_port),
        ))
    }
}

impl SockAddrIn6 {
    fn from_socket_addr(addr: &SocketAddr) -> Self {
        let ip = match addr {
            SocketAddr::V4(v4) if v4.ip().is_unspecified() => [0; 16],
            _ => {
                let mut loopback = [0; 16];
                loopback[15] = 1;
                loopback
            }
        };
        Self {
            sin6_family: AF_INET6,
            sin6_port: addr.port().to_be(),
            sin6_flowinfo: 0,
            sin6_addr: ip,
            sin6_scope_id: 0,
        }
    }
}

fn socket_from_fd(fd: usize) -> SysResult<Arc<dyn FileOp>> {
    let task = current_task().ok_or(Errno::ESRCH)?;
    let fd_entry = task.get_fd_entry(fd)?;
    if fd_entry.flags.contains(OpenFlags::O_PATH) {
        return Err(Errno::EBADF);
    }
    let file = fd_entry.file;
    if file.as_any().downcast_ref::<Socket>().is_none() {
        return Err(Errno::ENOTSOCK);
    }
    Ok(file)
}

fn with_socket<T>(fd: usize, f: impl FnOnce(&Socket) -> SysResult<T>) -> SysResult<T> {
    let file = socket_from_fd(fd)?;
    let socket = file
        .as_any()
        .downcast_ref::<Socket>()
        .ok_or(Errno::ENOTSOCK)?;
    f(socket)
}

fn read_sockaddr(addr: usize, len: usize) -> SysResult<SocketAddr> {
    if addr == 0 {
        return Err(Errno::EFAULT);
    }
    if len < core::mem::size_of::<SockAddrIn>() {
        return Err(Errno::EINVAL);
    }
    let mut sockaddr = SockAddrIn {
        sin_family: 0,
        sin_port: 0,
        sin_addr: [0; 4],
        sin_zero: [0; 8],
    };
    copy_from_user(
        &mut sockaddr as *mut SockAddrIn,
        addr as *const SockAddrIn,
        1,
    )?;
    if sockaddr.sin_family != AF_INET {
        return Err(Errno::EAFNOSUPPORT);
    }
    Ok(sockaddr.to_socket_addr())
}

fn read_sockaddr_for_domain(
    domain: &SocketDomain,
    addr: usize,
    len: usize,
) -> SysResult<SocketAddr> {
    if *domain != SocketDomain::AF_INET6 {
        return read_sockaddr(addr, len);
    }
    if addr == 0 {
        return Err(Errno::EFAULT);
    }
    if len < core::mem::size_of::<SockAddrIn6>() {
        return Err(Errno::EINVAL);
    }
    let mut sockaddr = SockAddrIn6 {
        sin6_family: 0,
        sin6_port: 0,
        sin6_flowinfo: 0,
        sin6_addr: [0; 16],
        sin6_scope_id: 0,
    };
    copy_from_user(
        &mut sockaddr as *mut SockAddrIn6,
        addr as *const SockAddrIn6,
        1,
    )?;
    if sockaddr.sin6_family != AF_INET6 {
        return Err(Errno::EAFNOSUPPORT);
    }
    let mut loopback = [0; 16];
    loopback[15] = 1;
    let ip = match sockaddr.sin6_addr {
        addr if addr == [0; 16] => Ipv4Addr::UNSPECIFIED,
        addr if addr == loopback => Ipv4Addr::LOCALHOST,
        _ => return Err(Errno::EADDRNOTAVAIL),
    };
    Ok(SocketAddr::V4(SocketAddrV4::new(
        ip,
        u16::from_be(sockaddr.sin6_port),
    )))
}

fn write_sockaddr(addr: usize, addrlen_ptr: usize, sockaddr: SocketAddr) -> SysResult {
    if addr == 0 {
        return Ok(());
    }
    let user_len = read_sockaddr_output_len(addrlen_ptr)?;
    let actual_len = core::mem::size_of::<SockAddrIn>() as u32;
    let raw = SockAddrIn::from_socket_addr(&sockaddr);
    let write_len = core::cmp::min(user_len as usize, core::mem::size_of::<SockAddrIn>());
    if write_len > 0 {
        check_user_writable(addr as *mut u8, write_len)?;
        copy_to_user(
            addr as *mut u8,
            &raw as *const SockAddrIn as *const u8,
            write_len,
        )?;
    }
    copy_to_user(addrlen_ptr as *mut u32, &actual_len as *const u32, 1)?;
    Ok(())
}

fn write_sockaddr_for_domain(
    domain: &SocketDomain,
    addr: usize,
    addrlen_ptr: usize,
    sockaddr: SocketAddr,
) -> SysResult {
    if *domain != SocketDomain::AF_INET6 {
        return write_sockaddr(addr, addrlen_ptr, sockaddr);
    }
    if addr == 0 {
        return Ok(());
    }
    let user_len = read_sockaddr_output_len(addrlen_ptr)?;
    let actual_len = core::mem::size_of::<SockAddrIn6>() as u32;
    let raw = SockAddrIn6::from_socket_addr(&sockaddr);
    let write_len = core::cmp::min(user_len as usize, core::mem::size_of::<SockAddrIn6>());
    if write_len > 0 {
        check_user_writable(addr as *mut u8, write_len)?;
        copy_to_user(
            addr as *mut u8,
            &raw as *const SockAddrIn6 as *const u8,
            write_len,
        )?;
    }
    copy_to_user(addrlen_ptr as *mut u32, &actual_len as *const u32, 1)?;
    Ok(())
}

fn read_sockaddr_output_len(addrlen_ptr: usize) -> SysResult<u32> {
    if addrlen_ptr < core::mem::size_of::<u32>() {
        return Err(Errno::EFAULT);
    }
    check_user_readable(addrlen_ptr as *const u32, 1)?;
    let mut user_len = 0u32;
    copy_from_user(&mut user_len as *mut u32, addrlen_ptr as *const u32, 1)?;
    if user_len as usize > 4096 {
        return Err(Errno::EINVAL);
    }
    Ok(user_len)
}

fn read_sockaddr_un(addr: usize, addrlen: usize) -> SysResult<SockAddrUn> {
    let min_len = core::mem::size_of::<u16>();
    if addr == 0 {
        return Err(Errno::EFAULT);
    }
    if addrlen < min_len {
        return Err(Errno::EINVAL);
    }
    let copy_len = core::cmp::min(addrlen, min_len + SOCKADDR_UN_PATH_LEN);
    let mut raw = vec![0u8; copy_len];
    copy_from_user(raw.as_mut_ptr(), addr as *const u8, copy_len)?;

    let family = u16::from_ne_bytes([raw[0], raw[1]]);
    if family != AF_UNIX {
        return Err(Errno::EAFNOSUPPORT);
    }

    let path_bytes = &raw[min_len..];
    if path_bytes.is_empty() {
        return Err(Errno::EINVAL);
    }
    if path_bytes[0] == 0 {
        let key = core::str::from_utf8(path_bytes)
            .map(String::from)
            .map_err(|_| Errno::EINVAL)?;
        return Ok(SockAddrUn {
            key,
            pathname: None,
        });
    }

    let end = path_bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(path_bytes.len());
    let path = core::str::from_utf8(&path_bytes[..end])
        .map(String::from)
        .map_err(|_| Errno::EINVAL)?;
    Ok(SockAddrUn {
        key: path.clone(),
        pathname: Some(path),
    })
}

fn create_unix_socket_node(path: &str) -> SysResult {
    filename_create(AT_FDCWD, path, InodeType::Socket, 0o777).map_err(|err| {
        if err == Errno::EEXIST {
            Errno::EADDRINUSE
        } else {
            err
        }
    })
}

fn write_sockaddr_un(addr: usize, addrlen_ptr: usize, key: Option<&str>) -> SysResult {
    if addr == 0 {
        return Ok(());
    }
    let user_len = read_sockaddr_output_len(addrlen_ptr)? as usize;
    let key_bytes = key.unwrap_or("").as_bytes();
    let actual_len = core::mem::size_of::<u16>() + key_bytes.len().min(SOCKADDR_UN_PATH_LEN);
    let write_len = core::cmp::min(user_len, actual_len);
    if write_len > 0 {
        check_user_writable(addr as *mut u8, write_len)?;
        let family = AF_UNIX.to_ne_bytes();
        let family_len = core::cmp::min(write_len, family.len());
        copy_to_user(addr as *mut u8, family.as_ptr(), family_len)?;
        if write_len > family.len() {
            copy_to_user(
                (addr + family.len()) as *mut u8,
                key_bytes.as_ptr(),
                write_len - family.len(),
            )?;
        }
    }
    let actual_len_u32 = actual_len as u32;
    copy_to_user(addrlen_ptr as *mut u32, &actual_len_u32 as *const u32, 1)?;
    Ok(())
}

fn is_local_ipv4(addr: &SocketAddr) -> bool {
    match addr {
        SocketAddr::V4(v4) => {
            let ip = v4.ip();
            ip.is_unspecified() || ip.octets()[0] == 127
        }
        SocketAddr::V6(_) => false,
    }
}

fn normalize_connect_addr(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V4(v4) if v4.ip().is_unspecified() => {
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, v4.port()))
        }
        _ => addr,
    }
}

fn write_sockaddr_value(addr: usize, addrlen: usize, sockaddr: SocketAddr) -> SysResult<u32> {
    if addr == 0 {
        return Ok(0);
    }
    let actual_len = core::mem::size_of::<SockAddrIn>();
    let raw = SockAddrIn::from_socket_addr(&sockaddr);
    let write_len = core::cmp::min(addrlen, actual_len);
    if write_len > 0 {
        check_user_writable(addr as *mut u8, write_len)?;
        copy_to_user(
            addr as *mut u8,
            &raw as *const SockAddrIn as *const u8,
            write_len,
        )?;
    }
    Ok(actual_len as u32)
}

fn read_msghdr(msg: usize) -> SysResult<MsgHdr> {
    if msg == 0 {
        return Err(Errno::EFAULT);
    }
    let mut hdr = MsgHdr {
        msg_name: 0,
        msg_namelen: 0,
        _pad1: 0,
        msg_iov: 0,
        msg_iovlen: 0,
        msg_control: 0,
        msg_controllen: 0,
        msg_flags: 0,
        _pad2: 0,
    };
    copy_from_user(&mut hdr as *mut MsgHdr, msg as *const MsgHdr, 1)?;
    if hdr.msg_iovlen > IOV_MAX {
        return Err(Errno::EMSGSIZE);
    }
    Ok(hdr)
}

fn write_msghdr(msg: usize, hdr: &MsgHdr) -> SysResult {
    copy_to_user(msg as *mut MsgHdr, hdr as *const MsgHdr, 1)?;
    Ok(())
}

fn read_mmsghdr(msg: usize, idx: usize) -> SysResult<MMsgHdr> {
    if msg == 0 {
        return Err(Errno::EFAULT);
    }
    let mut hdr = MMsgHdr {
        msg_hdr: MsgHdr {
            msg_name: 0,
            msg_namelen: 0,
            _pad1: 0,
            msg_iov: 0,
            msg_iovlen: 0,
            msg_control: 0,
            msg_controllen: 0,
            msg_flags: 0,
            _pad2: 0,
        },
        msg_len: 0,
    };
    copy_from_user(
        &mut hdr as *mut MMsgHdr,
        (msg as *const MMsgHdr).wrapping_add(idx),
        1,
    )?;
    if hdr.msg_hdr.msg_iovlen > IOV_MAX {
        return Err(Errno::EMSGSIZE);
    }
    Ok(hdr)
}

fn write_mmsghdr(msg: usize, idx: usize, hdr: &MMsgHdr) -> SysResult {
    copy_to_user(
        (msg as *mut MMsgHdr).wrapping_add(idx),
        hdr as *const MMsgHdr,
        1,
    )?;
    Ok(())
}

fn read_iovecs(iov: usize, iovcnt: usize) -> SysResult<Vec<MsgIov>> {
    if iovcnt == 0 {
        return Ok(Vec::new());
    }
    if iov == 0 {
        return Err(Errno::EFAULT);
    }
    let mut items = Vec::with_capacity(iovcnt);
    for idx in 0..iovcnt {
        let mut item = MsgIov {
            base: core::ptr::null_mut(),
            len: 0,
        };
        copy_from_user(
            &mut item as *mut MsgIov,
            (iov as *const MsgIov).wrapping_add(idx),
            1,
        )?;
        items.push(item);
    }
    Ok(items)
}

fn collect_iov_bytes(iovs: &[MsgIov]) -> SysResult<Vec<u8>> {
    let total = iovs.iter().try_fold(0usize, |acc, item| {
        acc.checked_add(item.len).ok_or(Errno::EINVAL)
    })?;
    let mut buf = Vec::with_capacity(total);
    for item in iovs {
        if item.len == 0 {
            continue;
        }
        let old_len = buf.len();
        buf.resize(old_len + item.len, 0);
        copy_from_user(
            buf[old_len..].as_mut_ptr(),
            item.base as *const u8,
            item.len,
        )?;
    }
    Ok(buf)
}

fn scatter_iov_bytes(iovs: &[MsgIov], data: &[u8]) -> SysResult<usize> {
    let mut copied = 0usize;
    for item in iovs {
        if copied >= data.len() {
            break;
        }
        if item.len == 0 {
            continue;
        }
        let chunk = core::cmp::min(item.len, data.len() - copied);
        copy_to_user(item.base, data[copied..].as_ptr(), chunk)?;
        copied += chunk;
    }
    Ok(copied)
}

fn read_i32(ptr: usize, len: usize) -> SysResult<i32> {
    if ptr == 0 {
        return Err(Errno::EFAULT);
    }
    if len < core::mem::size_of::<i32>() {
        return Err(Errno::EINVAL);
    }
    let mut value = 0i32;
    copy_from_user(&mut value as *mut i32, ptr as *const i32, 1)?;
    Ok(value)
}

fn read_u32(ptr: usize, len: usize) -> SysResult<u32> {
    if ptr == 0 {
        return Err(Errno::EFAULT);
    }
    if len < core::mem::size_of::<u32>() {
        return Err(Errno::EINVAL);
    }
    let mut value = 0u32;
    copy_from_user(&mut value as *mut u32, ptr as *const u32, 1)?;
    Ok(value)
}

fn read_timespec(ptr: usize, len: usize) -> SysResult<crate::timer::TimeSpec> {
    if ptr == 0 {
        return Err(Errno::EFAULT);
    }
    if len < core::mem::size_of::<crate::timer::TimeSpec>() {
        return Err(Errno::EINVAL);
    }
    let mut value = crate::timer::TimeSpec::default();
    copy_from_user(
        &mut value as *mut crate::timer::TimeSpec,
        ptr as *const crate::timer::TimeSpec,
        1,
    )?;
    if !value.is_valid_duration() {
        return Err(Errno::EINVAL);
    }
    Ok(value)
}

fn write_sockopt<T: Copy>(optval: usize, optlen: usize, value: &T) -> SysResult {
    if optval == 0 || optlen == 0 {
        return Err(Errno::EFAULT);
    }
    let mut user_len = 0u32;
    copy_from_user(&mut user_len as *mut u32, optlen as *const u32, 1)?;
    let actual_len = core::mem::size_of::<T>();
    if user_len as usize > 4096 {
        return Err(Errno::EINVAL);
    }
    let write_len = core::cmp::min(user_len as usize, actual_len);
    if write_len > 0 {
        check_user_writable(optval as *mut u8, write_len)?;
        copy_to_user(optval as *mut u8, value as *const T as *const u8, write_len)?;
    }
    let actual_len_u32 = actual_len as u32;
    copy_to_user(optlen as *mut u32, &actual_len_u32 as *const u32, 1)?;
    Ok(())
}

fn write_zero_sockopt(optval: usize, optlen: usize, actual_len: usize) -> SysResult {
    if optval == 0 || optlen == 0 {
        return Err(Errno::EFAULT);
    }
    let mut user_len = 0u32;
    copy_from_user(&mut user_len as *mut u32, optlen as *const u32, 1)?;
    if user_len as usize > 4096 {
        return Err(Errno::EINVAL);
    }
    let write_len = core::cmp::min(user_len as usize, actual_len);
    if write_len > 0 {
        check_user_writable(optval as *mut u8, write_len)?;
        let zeros = vec![0u8; write_len];
        copy_to_user(optval as *mut u8, zeros.as_ptr(), write_len)?;
    }
    let actual_len_u32 = actual_len as u32;
    copy_to_user(optlen as *mut u32, &actual_len_u32 as *const u32, 1)?;
    Ok(())
}

fn parse_socket(
    domain: usize,
    socket_type: usize,
    protocol: usize,
) -> SysResult<(SocketDomain, SocketKind)> {
    let domain = socket::parse_domain(domain)?;
    let kind = socket::parse_kind(socket_type)?;
    match (&domain, kind, protocol) {
        (
            SocketDomain::AF_INET | SocketDomain::AF_INET6,
            SocketKind::SOCK_STREAM,
            0 | IPPROTO_TCP | IPPROTO_SCTP,
        ) => {}
        (
            SocketDomain::AF_INET | SocketDomain::AF_INET6,
            SocketKind::SOCK_DGRAM,
            0 | IPPROTO_UDP,
        ) => {}
        (
            SocketDomain::AF_UNIX,
            SocketKind::SOCK_STREAM | SocketKind::SOCK_DGRAM | SocketKind::SOCK_SEQPACKET,
            0,
        ) => {}
        (_, SocketKind::SOCK_RAW, _) => return Err(Errno::EPROTONOSUPPORT),
        _ => return Err(Errno::EPROTONOSUPPORT),
    }
    Ok((domain, kind))
}

pub fn sys_socket(domain: usize, socket_type: usize, protocol: usize) -> SysResult<usize> {
    let (domain, kind) = parse_socket(domain, socket_type, protocol)?;
    let sock = Arc::new(Socket::new(domain, kind, protocol)?);
    sock.set_nonblocking((socket_type & SOCK_NONBLOCK) != 0);
    if (socket_type & SOCK_CLOEXEC) != 0 {
        sock.set_close_on_exec(true);
    }
    let task = current_task().ok_or(Errno::ESRCH)?;
    let flags = sock.get_flags();
    task.alloc_fd(FdEntry::new(sock, flags))
}

pub fn sys_socketpair(
    domain: usize,
    socket_type: usize,
    protocol: usize,
    sv: *mut i32,
) -> SysResult<usize> {
    let kind = socket::parse_kind(socket_type & SOCK_TYPE_MASK)?;
    if domain != AF_UNIX as usize {
        if domain != AF_INET as usize {
            return Err(Errno::EAFNOSUPPORT);
        }
        return match (kind, protocol) {
            (SocketKind::SOCK_DGRAM, IPPROTO_UDP) | (SocketKind::SOCK_STREAM, IPPROTO_TCP) => {
                Err(Errno::EOPNOTSUPP)
            }
            _ => Err(Errno::EPROTONOSUPPORT),
        };
    }
    if protocol != 0 {
        return Err(Errno::EPROTONOSUPPORT);
    }
    let (left, right) = Socket::new_unix_pair(kind)?;
    let nonblock = socket_type & SOCK_NONBLOCK != 0;
    let cloexec = socket_type & SOCK_CLOEXEC != 0;
    left.set_nonblocking(nonblock);
    right.set_nonblocking(nonblock);
    if cloexec {
        left.set_close_on_exec(true);
        right.set_close_on_exec(true);
    }

    check_user_writable(sv, 2)?;
    let left = Arc::new(left);
    let right = Arc::new(right);
    let task = current_task().ok_or(Errno::ESRCH)?;
    let left_fd = task.alloc_fd(FdEntry::new(left.clone(), left.get_flags()))?;
    let right_fd = match task.alloc_fd(FdEntry::new(right.clone(), right.get_flags())) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = task.close(left_fd);
            return Err(err);
        }
    };
    let fds = [left_fd as i32, right_fd as i32];
    if let Err(err) = copy_to_user(sv, fds.as_ptr(), fds.len()) {
        let _ = task.close(left_fd);
        let _ = task.close(right_fd);
        return Err(err);
    }
    Ok(0)
}

pub fn sys_bind(socketfd: usize, socketaddr: usize, socketlen: usize) -> SysResult<usize> {
    with_socket(socketfd, |sock| {
        if sock.domain == SocketDomain::AF_UNIX {
            let addr = read_sockaddr_un(socketaddr, socketlen)?;
            sock.ensure_unix_unbound()?;
            if let Some(path) = addr.pathname.as_ref() {
                create_unix_socket_node(path.as_str())?;
            }
            sock.bind_unix_path(addr.key.as_str())?;
            return Ok(0);
        }
        let addr = read_sockaddr_for_domain(&sock.domain, socketaddr, socketlen)?;
        if !is_local_ipv4(&addr) {
            return Err(Errno::EADDRNOTAVAIL);
        }
        if addr.port() < 1024 && current_task().ok_or(Errno::ESRCH)?.fsuid() != 0 {
            return Err(Errno::EACCES);
        }
        sock.bind(addr)?;
        Ok(0)
    })
}

pub fn sys_listen(socketfd: usize, _backlog: usize) -> SysResult<usize> {
    with_socket(socketfd, |sock| {
        sock.listen()?;
        Ok(0)
    })
}

pub fn sys_accept(socketfd: usize, addr: usize, addrlen: usize) -> SysResult<usize> {
    sys_accept4(socketfd, addr, addrlen, 0)
}

pub fn sys_accept4(socketfd: usize, addr: usize, addrlen: usize, flags: usize) -> SysResult<usize> {
    let allowed = SOCK_NONBLOCK | SOCK_CLOEXEC;
    if flags & !allowed != 0 {
        return Err(Errno::EINVAL);
    }
    let task = current_task().ok_or(Errno::ESRCH)?;
    let (new_sock, remote_addr) = with_socket(socketfd, |sock| {
        let (new_sock, remote_addr) = sock.accept()?;
        Ok((new_sock, remote_addr))
    })?;
    new_sock.set_nonblocking(flags & SOCK_NONBLOCK != 0);
    if flags & SOCK_CLOEXEC != 0 {
        new_sock.set_close_on_exec(true);
    }
    let fd_flags = new_sock.get_flags();
    let is_unix = new_sock.domain == SocketDomain::AF_UNIX;
    let domain = new_sock.domain.clone();
    let new_fd = task.alloc_fd(FdEntry::new(Arc::new(new_sock), fd_flags))?;
    let write_addr_result = if is_unix {
        write_sockaddr_un(addr, addrlen, None)
    } else {
        write_sockaddr_for_domain(&domain, addr, addrlen, remote_addr)
    };
    if let Err(err) = write_addr_result {
        let _ = task.close(new_fd);
        return Err(err);
    }
    Ok(new_fd)
}

pub fn sys_connect(socketfd: usize, addr: usize, addrlen: usize) -> SysResult<usize> {
    with_socket(socketfd, |sock| {
        if sock.domain == SocketDomain::AF_UNIX {
            let addr = read_sockaddr_un(addr, addrlen)?;
            sock.connect_unix_path(addr.key.as_str())?;
            return Ok(0);
        }
        let remote = normalize_connect_addr(read_sockaddr_for_domain(&sock.domain, addr, addrlen)?);
        sock.connect(remote)?;
        Ok(0)
    })
}

pub fn sys_getsockname(socketfd: usize, addr: usize, addrlen: usize) -> SysResult<usize> {
    if addr == 0 || addrlen == 0 {
        return Err(Errno::EFAULT);
    }
    with_socket(socketfd, |sock| {
        if sock.domain == SocketDomain::AF_UNIX {
            let key = sock.get_bound_unix_key()?;
            write_sockaddr_un(addr, addrlen, Some(key.as_str()))?;
            return Ok(0);
        }
        let bound = sock.get_bound_address()?;
        write_sockaddr_for_domain(&sock.domain, addr, addrlen, bound)?;
        Ok(0)
    })
}

pub fn sys_getpeername(socketfd: usize, addr: usize, addrlen: usize) -> SysResult<usize> {
    if addr == 0 || addrlen == 0 {
        return Err(Errno::EFAULT);
    }
    with_socket(socketfd, |sock| {
        let peer = sock.get_remote_addr()?;
        write_sockaddr_for_domain(&sock.domain, addr, addrlen, peer)?;
        Ok(0)
    })
}

pub fn sys_sendto(
    fd: usize,
    buf: *const u8,
    len: usize,
    _flags: usize,
    dest_addr: usize,
    addrlen: usize,
) -> SysResult<usize> {
    let mut kernel_buf: Vec<u8> = vec![0; len];
    copy_from_user(kernel_buf.as_mut_ptr(), buf, len)?;
    with_socket(fd, |sock| {
        if dest_addr != 0 {
            let remote = read_sockaddr_for_domain(&sock.domain, dest_addr, addrlen)?;
            sock.send_to(&kernel_buf, remote)
        } else {
            sock.write(&kernel_buf)
        }
    })
}

pub fn sys_recvfrom(
    fd: usize,
    buf: *mut u8,
    len: usize,
    _flags: usize,
    src_addr: usize,
    addrlen: usize,
) -> SysResult<usize> {
    let mut kernel_buf: Vec<u8> = vec![0; len];
    let (n, from_addr, domain) = with_socket(fd, |sock| {
        let (n, from_addr) = sock.recv_from(&mut kernel_buf)?;
        Ok((n, from_addr, sock.domain.clone()))
    })?;
    copy_to_user(buf, kernel_buf.as_ptr(), n.min(len))?;
    write_sockaddr_for_domain(&domain, src_addr, addrlen, from_addr)?;
    Ok(n.min(len))
}

pub fn sys_sendmsg(fd: usize, msg: usize, flags: usize) -> SysResult<usize> {
    let hdr = read_msghdr(msg)?;
    sys_sendmsg_from_hdr(fd, &hdr, flags)
}

fn sys_sendmsg_from_hdr(fd: usize, hdr: &MsgHdr, flags: usize) -> SysResult<usize> {
    if flags & (MSG_OOB | MSG_ERRQUEUE) != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    let iovs = read_iovecs(hdr.msg_iov, hdr.msg_iovlen)?;
    let data = collect_iov_bytes(&iovs)?;
    with_socket(fd, |sock| {
        if hdr.msg_name != 0 {
            let remote =
                read_sockaddr_for_domain(&sock.domain, hdr.msg_name, hdr.msg_namelen as usize)?;
            sock.send_to(&data, remote)
        } else {
            sock.write(&data)
        }
    })
}

pub fn sys_recvmsg(fd: usize, msg: usize, flags: usize) -> SysResult<usize> {
    let mut hdr = read_msghdr(msg)?;
    let copied = sys_recvmsg_into_hdr(fd, &mut hdr, flags)?;
    write_msghdr(msg, &hdr)?;
    Ok(copied)
}

fn sys_recvmsg_into_hdr(fd: usize, hdr: &mut MsgHdr, flags: usize) -> SysResult<usize> {
    if hdr.msg_namelen as usize > 4096 {
        return Err(Errno::EINVAL);
    }
    if flags & MSG_OOB != 0 {
        return Err(Errno::EINVAL);
    }
    if flags & MSG_ERRQUEUE != 0 {
        return Err(Errno::EAGAIN);
    }
    let iovs = read_iovecs(hdr.msg_iov, hdr.msg_iovlen)?;
    let total = iovs.iter().try_fold(0usize, |acc, item| {
        acc.checked_add(item.len).ok_or(Errno::EINVAL)
    })?;
    let mut data = vec![0; total];
    let (n, from_addr) = with_socket(fd, |sock| sock.recv_from(&mut data))?;
    let copied = scatter_iov_bytes(&iovs, &data[..n.min(data.len())])?;
    hdr.msg_namelen = write_sockaddr_value(hdr.msg_name, hdr.msg_namelen as usize, from_addr)?;
    hdr.msg_controllen = 0;
    hdr.msg_flags = 0;
    Ok(copied)
}

pub fn sys_sendmmsg(fd: usize, msgvec: usize, vlen: usize, flags: usize) -> SysResult<usize> {
    if vlen > IOV_MAX {
        return Err(Errno::EINVAL);
    }
    if vlen == 0 {
        return Ok(0);
    }
    let mut sent = 0usize;
    for idx in 0..vlen {
        let mut mmsg = read_mmsghdr(msgvec, idx)?;
        match sys_sendmsg_from_hdr(fd, &mmsg.msg_hdr, flags) {
            Ok(len) => {
                mmsg.msg_len = len as u32;
                write_mmsghdr(msgvec, idx, &mmsg)?;
                sent += 1;
            }
            Err(err) => {
                return if sent > 0 { Ok(sent) } else { Err(err) };
            }
        }
    }
    Ok(sent)
}

pub fn sys_recvmmsg(
    fd: usize,
    msgvec: usize,
    vlen: usize,
    flags: usize,
    timeout: usize,
) -> SysResult<usize> {
    if vlen > IOV_MAX {
        return Err(Errno::EINVAL);
    }
    if timeout != 0 {
        let mut ts = crate::timer::TimeSpec::default();
        copy_from_user(
            &mut ts as *mut crate::timer::TimeSpec,
            timeout as *const crate::timer::TimeSpec,
            1,
        )?;
        if !ts.is_valid_duration() {
            return Err(Errno::EINVAL);
        }
    }
    if vlen == 0 {
        return Ok(0);
    }
    let mut recvd = 0usize;
    for idx in 0..vlen {
        let mut mmsg = read_mmsghdr(msgvec, idx)?;
        match sys_recvmsg_into_hdr(fd, &mut mmsg.msg_hdr, flags) {
            Ok(len) => {
                mmsg.msg_len = len as u32;
                write_mmsghdr(msgvec, idx, &mmsg)?;
                recvd += 1;
            }
            Err(err) => {
                return if recvd > 0 { Ok(recvd) } else { Err(err) };
            }
        }
    }
    Ok(recvd)
}

pub fn sys_setsockopt(
    fd: usize,
    level: usize,
    optname: usize,
    optval: usize,
    optlen: usize,
) -> SysResult<usize> {
    with_socket(fd, |sock| match (level, optname) {
        (SOL_SOCKET, SO_REUSEADDR) => {
            sock.set_reuse_addr(read_i32(optval, optlen)? != 0);
            Ok(0)
        }
        (SOL_SOCKET, SO_OOBINLINE | SO_DONTROUTE | SO_BROADCAST | SO_KEEPALIVE | SO_REUSEPORT) => {
            let _ = read_i32(optval, optlen)?;
            Ok(0)
        }
        (SOL_SOCKET, SO_LINGER) => Ok(0),
        (SOL_SOCKET, SO_SNDBUF) => {
            let size = read_i32(optval, optlen)?;
            if size < 0 {
                return Err(Errno::EINVAL);
            }
            sock.set_send_buf_size(size as u64);
            Ok(0)
        }
        (SOL_SOCKET, SO_SNDBUFFORCE) => {
            let size = read_u32(optval, optlen)?;
            sock.set_send_buf_size(core::cmp::min(size, i32::MAX as u32) as u64);
            Ok(0)
        }
        (SOL_SOCKET, SO_RCVBUF) => {
            let size = read_i32(optval, optlen)?;
            if size < 0 {
                return Err(Errno::EINVAL);
            }
            sock.set_recv_buf_size(size as u64);
            Ok(0)
        }
        (SOL_SOCKET, SO_RCVBUFFORCE) => {
            let size = read_u32(optval, optlen)?;
            sock.set_recv_buf_size(core::cmp::min(size, i32::MAX as u32) as u64);
            Ok(0)
        }
        (SOL_SOCKET, SO_RCVTIMEO) => {
            sock.set_recv_timeout(read_timespec(optval, optlen)?);
            Ok(0)
        }
        (SOL_SOCKET, SO_SNDTIMEO) => {
            sock.set_send_timeout(read_timespec(optval, optlen)?);
            Ok(0)
        }
        (IPPROTO_TCP, TCP_NODELAY) => {
            sock.set_tcp_nodelay(read_i32(optval, optlen)? != 0)?;
            Ok(0)
        }
        (IPPROTO_IP, IP_TTL) => {
            let ttl = read_i32(optval, optlen)?;
            if !(1..=255).contains(&ttl) {
                return Err(Errno::EINVAL);
            }
            sock.set_hop_limit(ttl as u8)?;
            Ok(0)
        }
        (IPPROTO_IP, IP_RECVERR) => {
            let _ = read_i32(optval, optlen)?;
            Ok(0)
        }
        (IPPROTO_IPV6, IPV6_V6ONLY) => {
            let _ = read_i32(optval, optlen)?;
            Ok(0)
        }
        _ => Err(Errno::ENOPROTOOPT),
    })
}

pub fn sys_getsockopt(
    fd: usize,
    level: usize,
    optname: usize,
    optval: usize,
    optlen: usize,
) -> SysResult<usize> {
    with_socket(fd, |sock| {
        match (level, optname) {
            (SOL_SOCKET, SO_TYPE) => write_sockopt(optval, optlen, &sock.socket_type_value()),
            (SOL_SOCKET, SO_ERROR) => write_sockopt(optval, optlen, &0i32),
            (
                SOL_SOCKET,
                SO_OOBINLINE | SO_DONTROUTE | SO_BROADCAST | SO_KEEPALIVE | SO_REUSEPORT,
            ) => write_sockopt(optval, optlen, &0i32),
            (SOL_SOCKET, SO_LINGER) => write_sockopt(optval, optlen, &0i32),
            (SOL_SOCKET, SO_SNDBUF) => {
                let size = core::cmp::min(sock.send_buf_size(), i32::MAX as u64) as i32;
                write_sockopt(optval, optlen, &size)
            }
            (SOL_SOCKET, SO_RCVBUF) => {
                let size = core::cmp::min(sock.recv_buf_size(), i32::MAX as u64) as i32;
                write_sockopt(optval, optlen, &size)
            }
            (SOL_SOCKET, SO_RCVTIMEO) => {
                write_sockopt(optval, optlen, &sock.recv_timeout().unwrap_or_default())
            }
            (SOL_SOCKET, SO_SNDTIMEO) => {
                write_sockopt(optval, optlen, &sock.send_timeout().unwrap_or_default())
            }
            (IPPROTO_TCP, TCP_NODELAY) => {
                let value = if sock.tcp_nodelay()? { 1i32 } else { 0i32 };
                write_sockopt(optval, optlen, &value)
            }
            (IPPROTO_TCP, TCP_MAXSEG) => write_sockopt(optval, optlen, &TCP_DEFAULT_MAXSEG),
            (IPPROTO_TCP, TCP_INFO) => write_zero_sockopt(optval, optlen, 104),
            (IPPROTO_IPV6, IPV6_V6ONLY) => write_sockopt(optval, optlen, &0i32),
            (SOL_SOCKET, _)
            | (IPPROTO_IP, _)
            | (IPPROTO_IPV6, _)
            | (IPPROTO_TCP, _)
            | (IPPROTO_SCTP, _) => Err(Errno::ENOPROTOOPT),
            _ => Err(Errno::EOPNOTSUPP),
        }?;
        Ok(0)
    })
}

pub fn sys_shutdown(fd: usize, how: usize) -> SysResult<usize> {
    with_socket(fd, |sock| {
        sock.shutdown(how)?;
        Ok(0)
    })
}
