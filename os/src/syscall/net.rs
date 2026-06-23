use alloc::{sync::Arc, vec, vec::Vec};
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use crate::{
    fs::{FdEntry, FileOp},
    mm::{check_user_writable, copy_from_user, copy_to_user},
    net::socket::{self, SOCK_CLOEXEC, SOCK_NONBLOCK, Socket, SocketDomain, SocketKind},
    task::current_task,
};

use super::{Errno, SysResult};

const AF_INET: u16 = 2;
const AF_UNIX: usize = 1;
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
const SO_RCVTIMEO: usize = 20;
const SO_SNDTIMEO: usize = 21;
const SO_SNDBUFFORCE: usize = 32;
const SO_RCVBUFFORCE: usize = 33;
const IPPROTO_IP: usize = 0;
const IPPROTO_TCP: usize = 6;
const IPPROTO_UDP: usize = 17;
const IP_TTL: usize = 2;
const IP_RECVERR: usize = 11;
const TCP_NODELAY: usize = 1;
const TCP_MAXSEG: usize = 2;
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

fn socket_from_fd(fd: usize) -> SysResult<Arc<dyn FileOp>> {
    let task = current_task().ok_or(Errno::ESRCH)?;
    let file = task.get_fd_entry(fd)?.file;
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

fn write_sockaddr(addr: usize, addrlen_ptr: usize, sockaddr: SocketAddr) -> SysResult {
    if addr == 0 {
        return Ok(());
    }
    if addrlen_ptr == 0 {
        return Err(Errno::EFAULT);
    }
    let actual_len = core::mem::size_of::<SockAddrIn>() as u32;
    let mut user_len = 0u32;
    copy_from_user(&mut user_len as *mut u32, addrlen_ptr as *const u32, 1)?;
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
        return Err(Errno::EINVAL);
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
        return Err(Errno::EINVAL);
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

fn parse_socket(
    domain: usize,
    socket_type: usize,
    protocol: usize,
) -> SysResult<(SocketDomain, SocketKind)> {
    let domain = socket::parse_domain(domain)?;
    let kind = socket::parse_kind(socket_type)?;
    match (&domain, kind, protocol) {
        (SocketDomain::AF_INET, SocketKind::SOCK_STREAM, 0 | IPPROTO_TCP) => {}
        (SocketDomain::AF_INET, SocketKind::SOCK_DGRAM, 0 | IPPROTO_UDP) => {}
        (SocketDomain::AF_UNIX, SocketKind::SOCK_STREAM | SocketKind::SOCK_DGRAM, 0) => {}
        (SocketDomain::AF_INET6, _, _) => return Err(Errno::EAFNOSUPPORT),
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
    if domain != AF_UNIX {
        return if domain == AF_INET as usize {
            Err(Errno::EOPNOTSUPP)
        } else {
            Err(Errno::EAFNOSUPPORT)
        };
    }
    if protocol != 0 {
        return Err(Errno::EPROTONOSUPPORT);
    }
    let kind = socket::parse_kind(socket_type & SOCK_TYPE_MASK)?;
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
            return Ok(0);
        }
        let addr = read_sockaddr(socketaddr, socketlen)?;
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
    let new_fd = task.alloc_fd(FdEntry::new(Arc::new(new_sock), fd_flags))?;
    if let Err(err) = write_sockaddr(addr, addrlen, remote_addr) {
        let _ = task.close(new_fd);
        return Err(err);
    }
    Ok(new_fd)
}

pub fn sys_connect(socketfd: usize, addr: usize, addrlen: usize) -> SysResult<usize> {
    with_socket(socketfd, |sock| {
        if sock.domain == SocketDomain::AF_UNIX {
            if addr == 0 {
                return Err(Errno::EFAULT);
            }
            if addrlen < core::mem::size_of::<u16>() {
                return Err(Errno::EINVAL);
            }
            return Err(Errno::ENOENT);
        }
        let remote = read_sockaddr(addr, addrlen)?;
        sock.connect(remote)?;
        Ok(0)
    })
}

pub fn sys_getsockname(socketfd: usize, addr: usize, addrlen: usize) -> SysResult<usize> {
    if addr == 0 || addrlen == 0 {
        return Err(Errno::EFAULT);
    }
    with_socket(socketfd, |sock| {
        let bound = sock.get_bound_address()?;
        write_sockaddr(addr, addrlen, bound)?;
        Ok(0)
    })
}

pub fn sys_getpeername(socketfd: usize, addr: usize, addrlen: usize) -> SysResult<usize> {
    if addr == 0 || addrlen == 0 {
        return Err(Errno::EFAULT);
    }
    with_socket(socketfd, |sock| {
        let peer = sock.get_remote_addr()?;
        write_sockaddr(addr, addrlen, peer)?;
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
            let remote = read_sockaddr(dest_addr, addrlen)?;
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
    let (n, from_addr) = with_socket(fd, |sock| sock.recv_from(&mut kernel_buf))?;
    copy_to_user(buf, kernel_buf.as_ptr(), n.min(len))?;
    write_sockaddr(src_addr, addrlen, from_addr)?;
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
            let remote = read_sockaddr(hdr.msg_name, hdr.msg_namelen as usize)?;
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
    if flags & (MSG_OOB | MSG_ERRQUEUE) != 0 {
        return Err(Errno::EOPNOTSUPP);
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
        (SOL_SOCKET, SO_OOBINLINE | SO_DONTROUTE | SO_BROADCAST | SO_KEEPALIVE) => {
            let _ = read_i32(optval, optlen)?;
            Ok(0)
        }
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
            (SOL_SOCKET, SO_OOBINLINE | SO_DONTROUTE | SO_BROADCAST | SO_KEEPALIVE) => {
                write_sockopt(optval, optlen, &0i32)
            }
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
            (SOL_SOCKET, _) | (IPPROTO_IP, _) | (IPPROTO_TCP, _) => Err(Errno::ENOPROTOOPT),
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
