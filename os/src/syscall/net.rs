use super::{Errno, SysResult};
use crate::fs::vfs::InodeType;
use crate::fs::{FdEntry, FileOp, KStat, OpenFlags};
use crate::mm::{copy_from_user, copy_to_user};
use crate::mutex::SpinLock;
use crate::task::{current_task, yield_current_task};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOCK_DGRAM: usize = 2;
const SOCK_TYPE_MASK: usize = 0xf;
const SOCK_NONBLOCK: usize = 0x800;
const SOCK_CLOEXEC: usize = 0x80000;

const SOL_SOCKET: usize = 1;
const SO_RCVTIMEO: usize = 20;

const IPPROTO_TCP: usize = 6;
const IPPROTO_UDP: usize = 17;

const LOOPBACK_ADDR: u32 = 0x0100007f;

static NEXT_EPHEMERAL_PORT: AtomicUsize = AtomicUsize::new(49152);

lazy_static! {
    static ref LOOPBACK: SpinLock<LoopbackState> = SpinLock::new(LoopbackState::new());
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn any(port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: 0,
            sin_zero: [0; 8],
        }
    }

    fn loopback(port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: LOOPBACK_ADDR,
            sin_zero: [0; 8],
        }
    }

    fn port(&self) -> u16 {
        u16::from_be(self.sin_port)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SocketKind {
    Stream,
    Datagram,
}

struct SocketInner {
    kind: SocketKind,
    local_port: u16,
    nonblocking: bool,
    cloexec: bool,
    listening: bool,
}

pub struct SocketFile {
    inner: SpinLock<SocketInner>,
}

impl SocketFile {
    fn new(kind: SocketKind, nonblocking: bool, cloexec: bool) -> Self {
        Self {
            inner: SpinLock::new(SocketInner {
                kind,
                local_port: 0,
                nonblocking,
                cloexec,
                listening: false,
            }),
        }
    }

    fn fd_flags(&self) -> OpenFlags {
        let inner = self.inner.lock();
        let mut flags = OpenFlags::O_RDWR;
        if inner.nonblocking {
            flags |= OpenFlags::O_NONBLOCK;
        }
        if inner.cloexec {
            flags |= OpenFlags::O_CLOEXEC;
        }
        flags
    }

    fn is_nonblocking(&self) -> bool {
        self.inner.lock().nonblocking
    }
}

impl FileOp for SocketFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, _buf: &'a mut [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn write<'a>(&'a self, _buf: &'a [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn get_flags(&self) -> OpenFlags {
        self.fd_flags()
    }

    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Socket))
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }
}

struct UdpPacket {
    data: Vec<u8>,
    peer: SockAddrIn,
}

struct LoopbackState {
    udp_queues: Vec<(u16, VecDeque<UdpPacket>)>,
    tcp_listeners: Vec<(u16, VecDeque<Arc<SocketFile>>)>,
}

impl LoopbackState {
    fn new() -> Self {
        Self {
            udp_queues: Vec::new(),
            tcp_listeners: Vec::new(),
        }
    }

    fn udp_queue_mut(&mut self, port: u16) -> &mut VecDeque<UdpPacket> {
        if let Some(idx) = self.udp_queues.iter().position(|(p, _)| *p == port) {
            return &mut self.udp_queues[idx].1;
        }
        self.udp_queues.push((port, VecDeque::new()));
        &mut self.udp_queues.last_mut().unwrap().1
    }

    fn tcp_listener_mut(&mut self, port: u16) -> &mut VecDeque<Arc<SocketFile>> {
        if let Some(idx) = self.tcp_listeners.iter().position(|(p, _)| *p == port) {
            return &mut self.tcp_listeners[idx].1;
        }
        self.tcp_listeners.push((port, VecDeque::new()));
        &mut self.tcp_listeners.last_mut().unwrap().1
    }

    fn tcp_listener_exists(&self, port: u16) -> bool {
        self.tcp_listeners.iter().any(|(p, _)| *p == port)
    }
}

fn next_port() -> u16 {
    let port = NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::Relaxed);
    49152 + ((port - 49152) % 16384) as u16
}

fn socket_from_fd(fd: usize) -> SysResult<Arc<dyn FileOp>> {
    let task = current_task().expect("[kernel] current task is None.");
    let file = task.get_fd_entry(fd)?.file;
    if file.as_any().downcast_ref::<SocketFile>().is_none() {
        return Err(Errno::EBADF);
    }
    Ok(file)
}

fn with_socket<T>(fd: usize, f: impl FnOnce(&SocketFile) -> SysResult<T>) -> SysResult<T> {
    let file = socket_from_fd(fd)?;
    let socket = file
        .as_any()
        .downcast_ref::<SocketFile>()
        .ok_or(Errno::EBADF)?;
    f(socket)
}

fn read_sockaddr(addr: usize, len: usize) -> SysResult<SockAddrIn> {
    if len < core::mem::size_of::<SockAddrIn>() {
        return Err(Errno::EINVAL);
    }
    let mut sockaddr = SockAddrIn::any(0);
    copy_from_user(
        &mut sockaddr as *mut SockAddrIn,
        addr as *const SockAddrIn,
        1,
    )?;
    if sockaddr.sin_family as usize != AF_INET {
        return Err(Errno::EINVAL);
    }
    Ok(sockaddr)
}

fn write_sockaddr(addr: usize, len_ptr: usize, sockaddr: SockAddrIn) -> SysResult {
    if addr != 0 {
        copy_to_user(addr as *mut SockAddrIn, &sockaddr as *const SockAddrIn, 1)?;
    }
    if len_ptr != 0 {
        let len = core::mem::size_of::<SockAddrIn>() as u32;
        copy_to_user(len_ptr as *mut u32, &len as *const u32, 1)?;
    }
    Ok(())
}

pub fn sys_socket(domain: usize, socket_type: usize, protocol: usize) -> SysResult<usize> {
    if domain != AF_INET {
        return Err(Errno::EINVAL);
    }

    let kind = match socket_type & SOCK_TYPE_MASK {
        SOCK_STREAM if protocol == 0 || protocol == IPPROTO_TCP => SocketKind::Stream,
        SOCK_DGRAM if protocol == 0 || protocol == IPPROTO_UDP => SocketKind::Datagram,
        _ => return Err(Errno::EINVAL),
    };

    let nonblocking = socket_type & SOCK_NONBLOCK != 0;
    let cloexec = socket_type & SOCK_CLOEXEC != 0;
    let socket = Arc::new(SocketFile::new(kind, nonblocking, cloexec));
    let flags = socket.fd_flags();
    let task = current_task().expect("[kernel] current task is None.");
    task.alloc_fd(FdEntry::new(socket, flags))
}

pub fn sys_bind(fd: usize, addr: usize, len: usize) -> SysResult<usize> {
    let sockaddr = read_sockaddr(addr, len)?;
    with_socket(fd, |socket| {
        let mut inner = socket.inner.lock();
        let port = if sockaddr.port() == 0 {
            next_port()
        } else {
            sockaddr.port()
        };
        inner.local_port = port;
        if inner.kind == SocketKind::Datagram {
            LOOPBACK.lock().udp_queue_mut(port);
        }
        Ok(0)
    })
}

pub fn sys_getsockname(fd: usize, addr: usize, len_ptr: usize) -> SysResult<usize> {
    with_socket(fd, |socket| {
        let port = socket.inner.lock().local_port;
        write_sockaddr(addr, len_ptr, SockAddrIn::any(port))?;
        Ok(0)
    })
}

pub fn sys_setsockopt(
    fd: usize,
    level: usize,
    optname: usize,
    _optval: usize,
    _optlen: usize,
) -> SysResult<usize> {
    socket_from_fd(fd)?;
    if level == SOL_SOCKET && optname == SO_RCVTIMEO {
        return Ok(0);
    }
    Ok(0)
}

pub fn sys_sendto(
    fd: usize,
    buf: *const u8,
    len: usize,
    _flags: usize,
    addr: usize,
    addr_len: usize,
) -> SysResult<usize> {
    let dst = read_sockaddr(addr, addr_len)?;
    with_socket(fd, |socket| {
        if socket.inner.lock().kind != SocketKind::Datagram {
            return Err(Errno::EINVAL);
        }
        let mut data = alloc::vec![0u8; len];
        copy_from_user(data.as_mut_ptr(), buf, len)?;
        let src_port = {
            let mut inner = socket.inner.lock();
            if inner.local_port == 0 {
                inner.local_port = next_port();
            }
            inner.local_port
        };
        let packet = UdpPacket {
            data,
            peer: SockAddrIn::loopback(src_port),
        };
        LOOPBACK.lock().udp_queue_mut(dst.port()).push_back(packet);
        Ok(len)
    })
}

pub fn sys_recvfrom(
    fd: usize,
    buf: *mut u8,
    len: usize,
    _flags: usize,
    addr: usize,
    addr_len: usize,
) -> SysResult<usize> {
    loop {
        let packet = with_socket(fd, |socket| {
            let inner = socket.inner.lock();
            if inner.kind != SocketKind::Datagram || inner.local_port == 0 {
                return Err(Errno::EINVAL);
            }
            Ok(LOOPBACK.lock().udp_queue_mut(inner.local_port).pop_front())
        })?;

        if let Some(packet) = packet {
            let read_len = len.min(packet.data.len());
            copy_to_user(buf, packet.data.as_ptr(), read_len)?;
            write_sockaddr(addr, addr_len, packet.peer)?;
            return Ok(read_len);
        }
        if with_socket(fd, |socket| Ok(socket.is_nonblocking()))? {
            return Err(Errno::EAGAIN);
        }
        yield_current_task();
    }
}

pub fn sys_listen(fd: usize, _backlog: usize) -> SysResult<usize> {
    with_socket(fd, |socket| {
        let mut inner = socket.inner.lock();
        if inner.kind != SocketKind::Stream {
            return Err(Errno::EINVAL);
        }
        if inner.local_port == 0 {
            inner.local_port = next_port();
        }
        inner.listening = true;
        LOOPBACK.lock().tcp_listener_mut(inner.local_port);
        Ok(0)
    })
}

pub fn sys_connect(fd: usize, addr: usize, len: usize) -> SysResult<usize> {
    let dst = read_sockaddr(addr, len)?;
    with_socket(fd, |socket| {
        {
            let mut inner = socket.inner.lock();
            if inner.kind != SocketKind::Stream {
                return Err(Errno::EINVAL);
            }
            if inner.local_port == 0 {
                inner.local_port = next_port();
            }
        }
        if !LOOPBACK.lock().tcp_listener_exists(dst.port()) {
            return Err(Errno::ECONNREFUSED);
        }
        let accepted = Arc::new(SocketFile::new(SocketKind::Stream, false, false));
        accepted.inner.lock().local_port = dst.port();
        LOOPBACK
            .lock()
            .tcp_listener_mut(dst.port())
            .push_back(accepted);
        Ok(0)
    })
}

pub fn sys_accept(fd: usize, addr: usize, addr_len: usize) -> SysResult<usize> {
    loop {
        let accepted = with_socket(fd, |socket| {
            let inner = socket.inner.lock();
            if inner.kind != SocketKind::Stream || !inner.listening {
                return Err(Errno::EINVAL);
            }
            Ok(LOOPBACK
                .lock()
                .tcp_listener_mut(inner.local_port)
                .pop_front())
        })?;

        if let Some(accepted) = accepted {
            let port = accepted.inner.lock().local_port;
            write_sockaddr(addr, addr_len, SockAddrIn::loopback(port))?;
            let flags = accepted.fd_flags();
            let task = current_task().expect("[kernel] current task is None.");
            return task.alloc_fd(FdEntry::new(accepted, flags));
        }
        if with_socket(fd, |socket| Ok(socket.is_nonblocking()))? {
            return Err(Errno::EAGAIN);
        }
        yield_current_task();
    }
}
