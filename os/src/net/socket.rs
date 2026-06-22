//! 套接字抽象层。
//!
//! `Socket` 是用户态可见的套接字对象，实现了 RespOS 的 `FileOp` trait，
//! 从而可以通过标准文件描述符接口（read/write/poll）操作。
//! 内部根据 `SocketKind` 分派到 `TcpSocket` 或 `UdpSocket`。

use alloc::{collections::VecDeque, sync::Arc};
use core::{
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use spin::Mutex;

use crate::{
    fs::vfs::InodeType,
    fs::{FileOp, KStat, OpenFlags},
    mutex::SpinLock,
    syscall::{Errno, SysResult},
    task::yield_current_task,
};

use super::{
    addr::{UNSPECIFIED_ENDPOINT, from_ipendpoint_to_socketaddr},
    poll_interfaces,
    tcp::TcpSocket,
    udp::UdpSocket,
};

const UNIX_SOCKET_BUFFER_LIMIT: usize = 64 * 1024;

// ——— 类型枚举 ———

/// 套接字地址族。
#[allow(non_camel_case_types)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SocketDomain {
    /// UNIX 域套接字（暂不支持完整语义）。
    AF_UNIX = 1,
    /// IPv4 套接字。
    AF_INET = 2,
    /// IPv6 套接字（暂未实现）。
    AF_INET6 = 10,
}

/// 套接字类型。
#[allow(non_camel_case_types)]
#[derive(Clone, PartialEq, Eq, Debug, Copy)]
pub enum SocketKind {
    /// 流式套接字（TCP）。
    SOCK_STREAM = 1,
    /// 数据报套接字（UDP）。
    SOCK_DGRAM = 2,
    /// 原始套接字（暂不支持）。
    SOCK_RAW = 3,
}

// ——— SocketInner ———

/// 内部协议套接字，分派到 TCP 或 UDP。
enum SocketInner {
    Tcp(TcpSocket),
    Udp(UdpSocket),
    Unix(UnixSocket),
}

struct UnixSocket {
    rx: Arc<SpinLock<VecDeque<u8>>>,
    peer_rx: SpinLock<Option<Arc<SpinLock<VecDeque<u8>>>>>,
    nonblock: AtomicBool,
}

impl UnixSocket {
    fn new() -> Self {
        Self {
            rx: Arc::new(SpinLock::new(VecDeque::new())),
            peer_rx: SpinLock::new(None),
            nonblock: AtomicBool::new(false),
        }
    }

    fn pair() -> (Self, Self) {
        let left = Self::new();
        let right = Self::new();
        *left.peer_rx.lock() = Some(right.rx.clone());
        *right.peer_rx.lock() = Some(left.rx.clone());
        (left, right)
    }

    fn set_nonblocking(&self, nonblock: bool) {
        self.nonblock.store(nonblock, Ordering::Release);
    }

    fn is_nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.peer_rx.lock().is_none() {
            return Err(Errno::ENOTCONN);
        }
        loop {
            let mut rx = self.rx.lock();
            if !rx.is_empty() {
                let mut read_len = 0;
                for byte in buf {
                    let Some(value) = rx.pop_front() else {
                        break;
                    };
                    *byte = value;
                    read_len += 1;
                }
                return Ok(read_len);
            }
            drop(rx);
            if self.is_nonblocking() {
                return Err(Errno::EAGAIN);
            }
            yield_current_task();
        }
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let peer_rx = self.peer_rx.lock().clone().ok_or(Errno::ENOTCONN)?;
        loop {
            let mut rx = peer_rx.lock();
            let available = UNIX_SOCKET_BUFFER_LIMIT.saturating_sub(rx.len());
            if available > 0 {
                let write_len = available.min(buf.len());
                rx.extend(buf[..write_len].iter().copied());
                return Ok(write_len);
            }
            drop(rx);
            if self.is_nonblocking() {
                return Err(Errno::EAGAIN);
            }
            yield_current_task();
        }
    }

    fn read_ready(&self) -> bool {
        !self.rx.lock().is_empty()
    }

    fn write_ready(&self) -> bool {
        self.peer_rx
            .lock()
            .as_ref()
            .is_some_and(|rx| rx.lock().len() < UNIX_SOCKET_BUFFER_LIMIT)
    }
}

// ——— Socket ———

/// 用户态可见的套接字对象。
///
/// 实现 `FileOp`，可存入 fd_table 并通过 read/write 等系统调用操作。
pub struct Socket {
    /// 地址族（AF_INET / AF_UNIX / AF_INET6）。
    pub domain: SocketDomain,
    /// 套接字类型（SOCK_STREAM / SOCK_DGRAM）。
    pub kind: SocketKind,
    /// 内部协议实现。
    inner: SocketInner,
    /// 非阻塞标志。
    nonblock: AtomicBool,
    /// close-on-exec 标志。
    cloexec: AtomicBool,
    /// SO_SNDBUF 值。
    #[allow(dead_code)]
    send_buf_size: AtomicU64,
    /// SO_RCVBUF 值。
    #[allow(dead_code)]
    recv_buf_size: AtomicU64,
    /// SO_RCVTIMEO 值。
    #[allow(dead_code)]
    recvtimeout: Mutex<Option<crate::timer::TimeSpec>>,
    /// SO_SNDTIMEO 值。
    #[allow(dead_code)]
    sendtimeout: Mutex<Option<crate::timer::TimeSpec>>,
}

// SAFETY: 单核协作式调度，方法调用在系统调用路径上串行化。
unsafe impl Send for Socket {}
unsafe impl Sync for Socket {}

impl Socket {
    /// 创建一个新的套接字。
    pub fn new(
        domain: SocketDomain,
        socket_type: SocketKind,
        _protocol: usize,
    ) -> Result<Self, Errno> {
        let inner = match (&domain, socket_type) {
            (SocketDomain::AF_UNIX, SocketKind::SOCK_STREAM | SocketKind::SOCK_DGRAM) => {
                SocketInner::Unix(UnixSocket::new())
            }
            (SocketDomain::AF_INET, SocketKind::SOCK_STREAM) => SocketInner::Tcp(TcpSocket::new()),
            (SocketDomain::AF_INET, SocketKind::SOCK_DGRAM) => SocketInner::Udp(UdpSocket::new()),
            (SocketDomain::AF_INET6, _) => return Err(Errno::EAFNOSUPPORT),
            (_, SocketKind::SOCK_RAW) => return Err(Errno::EPROTONOSUPPORT),
        };
        Ok(Socket {
            domain,
            kind: socket_type,
            inner,
            nonblock: AtomicBool::new(false),
            cloexec: AtomicBool::new(false),
            send_buf_size: AtomicU64::new(64 * 1024),
            recv_buf_size: AtomicU64::new(64 * 1024),
            recvtimeout: Mutex::new(None),
            sendtimeout: Mutex::new(None),
        })
    }

    pub fn new_unix_pair(socket_type: SocketKind) -> Result<(Self, Self), Errno> {
        if !matches!(
            socket_type,
            SocketKind::SOCK_STREAM | SocketKind::SOCK_DGRAM
        ) {
            return Err(Errno::EINVAL);
        }
        let (left, right) = UnixSocket::pair();
        let make = |inner| Socket {
            domain: SocketDomain::AF_UNIX,
            kind: socket_type,
            inner: SocketInner::Unix(inner),
            nonblock: AtomicBool::new(false),
            cloexec: AtomicBool::new(false),
            send_buf_size: AtomicU64::new(64 * 1024),
            recv_buf_size: AtomicU64::new(64 * 1024),
            recvtimeout: Mutex::new(None),
            sendtimeout: Mutex::new(None),
        };
        Ok((make(left), make(right)))
    }

    pub fn set_nonblocking(&self, block: bool) {
        self.nonblock.store(block, Ordering::Release);
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.set_nonblocking(block),
            SocketInner::Udp(udp) => udp.set_nonblocking(block),
            SocketInner::Unix(unix) => unix.set_nonblocking(block),
        }
    }

    pub fn is_nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    /// 设置 FD_CLOEXEC 标志。
    pub fn set_close_on_exec(&self, is_set: bool) {
        self.cloexec.store(is_set, Ordering::Release);
    }

    pub fn socket_type_value(&self) -> i32 {
        self.kind as i32
    }

    pub fn set_reuse_addr(&self, reuse: bool) {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.set_reuse_addr(reuse),
            SocketInner::Udp(udp) => udp.set_reuse_addr(reuse),
            SocketInner::Unix(_) => {}
        }
    }

    pub fn set_send_buf_size(&self, size: u64) {
        self.send_buf_size.store(size, Ordering::Release);
    }

    pub fn send_buf_size(&self) -> u64 {
        self.send_buf_size.load(Ordering::Acquire)
    }

    pub fn set_recv_buf_size(&self, size: u64) {
        self.recv_buf_size.store(size, Ordering::Release);
    }

    pub fn recv_buf_size(&self) -> u64 {
        self.recv_buf_size.load(Ordering::Acquire)
    }

    pub fn set_recv_timeout(&self, timeout: crate::timer::TimeSpec) {
        *self.recvtimeout.lock() = Some(timeout);
    }

    pub fn recv_timeout(&self) -> Option<crate::timer::TimeSpec> {
        *self.recvtimeout.lock()
    }

    pub fn set_send_timeout(&self, timeout: crate::timer::TimeSpec) {
        *self.sendtimeout.lock() = Some(timeout);
    }

    pub fn send_timeout(&self) -> Option<crate::timer::TimeSpec> {
        *self.sendtimeout.lock()
    }

    pub fn set_tcp_nodelay(&self, enabled: bool) -> SysResult {
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                tcp.set_nagle_enabled(!enabled);
                Ok(())
            }
            _ => Err(Errno::ENOPROTOOPT),
        }
    }

    pub fn tcp_nodelay(&self) -> SysResult<bool> {
        match &self.inner {
            SocketInner::Tcp(tcp) => Ok(!tcp.nagle_enabled()),
            _ => Err(Errno::ENOPROTOOPT),
        }
    }

    pub fn set_hop_limit(&self, limit: u8) -> SysResult {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.set_hop_limit(limit),
            SocketInner::Udp(udp) => {
                udp.set_socket_ttl(limit);
                Ok(())
            }
            SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 获取绑定地址（getsockname）。
    pub fn get_bound_address(&self) -> Result<SocketAddr, Errno> {
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let local_addr = tcp.local_addr()?;
                Ok(from_ipendpoint_to_socketaddr(local_addr))
            }
            SocketInner::Udp(udp) => udp.local_addr(),
            SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 获取对端地址（getpeername）。
    pub fn get_remote_addr(&self) -> Result<SocketAddr, Errno> {
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let remote_addr = tcp.remote_addr()?;
                Ok(from_ipendpoint_to_socketaddr(remote_addr))
            }
            SocketInner::Udp(udp) => udp.remote_addr(),
            SocketInner::Unix(_) => Err(Errno::ENOTCONN),
        }
    }

    /// 绑定到本地地址。
    pub fn bind(&self, local_addr: SocketAddr) -> SysResult {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.bind(local_addr),
            SocketInner::Udp(udp) => udp.bind(local_addr),
            SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 开始监听（仅 TCP）。
    pub fn listen(&self) -> SysResult {
        if !matches!(self.kind, SocketKind::SOCK_STREAM) {
            return Err(Errno::EOPNOTSUPP);
        }
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.listen(),
            SocketInner::Udp(_) | SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 接受入站连接（仅 TCP），返回新的已连接 Socket 和对端地址。
    pub fn accept(&self) -> Result<(Self, SocketAddr), Errno> {
        if !matches!(self.kind, SocketKind::SOCK_STREAM) {
            return Err(Errno::EOPNOTSUPP);
        }
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let new_tcp = tcp.accept()?;
                let remote_addr = match new_tcp.remote_addr() {
                    Ok(a) => a,
                    Err(_) => UNSPECIFIED_ENDPOINT,
                };
                Ok((
                    Socket {
                        domain: self.domain.clone(),
                        kind: self.kind,
                        inner: SocketInner::Tcp(new_tcp),
                        nonblock: AtomicBool::new(false),
                        cloexec: AtomicBool::new(false),
                        send_buf_size: AtomicU64::new(64 * 1024),
                        recv_buf_size: AtomicU64::new(64 * 1024),
                        recvtimeout: Mutex::new(None),
                        sendtimeout: Mutex::new(None),
                    },
                    from_ipendpoint_to_socketaddr(remote_addr),
                ))
            }
            SocketInner::Udp(_) | SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 连接到远程地址。
    pub fn connect(&self, addr: SocketAddr) -> Result<(), Errno> {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.connect(addr),
            SocketInner::Udp(udp) => udp.connect(addr),
            SocketInner::Unix(_) => Err(Errno::ENOENT),
        }
    }

    /// 关闭套接字的一端或两端。
    pub fn shutdown(&self, how: usize) -> SysResult {
        if how > 2 {
            return Err(Errno::EINVAL);
        }
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                tcp.shutdown(how)?;
            }
            SocketInner::Udp(udp) => {
                udp.shutdown();
            }
            SocketInner::Unix(_) => {}
        }
        Ok(())
    }

    /// 向指定地址发送数据（sendto）。
    pub fn send_to(&self, buf: &[u8], addr: SocketAddr) -> Result<usize, Errno> {
        match &self.inner {
            SocketInner::Udp(udp) => udp.send_to(buf, addr),
            SocketInner::Tcp(tcp) => tcp.send(buf),
            SocketInner::Unix(unix) => unix.write(buf),
        }
    }

    /// 接收数据并返回发送方地址（recvfrom）。
    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), Errno> {
        match &self.inner {
            SocketInner::Udp(udp) => udp.recv_from(buf),
            SocketInner::Tcp(tcp) => {
                let len = tcp.recv(buf)?;
                let remote_addr = tcp.remote_addr().unwrap_or(UNSPECIFIED_ENDPOINT);
                Ok((len, from_ipendpoint_to_socketaddr(remote_addr)))
            }
            SocketInner::Unix(unix) => {
                let len = unix.read(buf)?;
                Ok((len, from_ipendpoint_to_socketaddr(UNSPECIFIED_ENDPOINT)))
            }
        }
    }

    /// 查询可读或可写状态（内部 poll + 状态检查）。
    fn tcp_poll(&self, isread: bool) -> bool {
        poll_interfaces();
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let state = tcp.poll(isread);
                if isread {
                    state.readable
                } else {
                    state.writeable
                }
            }
            SocketInner::Udp(udp) => {
                let state = udp.poll();
                if isread {
                    state.readable
                } else {
                    state.writeable
                }
            }
            SocketInner::Unix(unix) => {
                if isread {
                    unix.read_ready()
                } else {
                    unix.write_ready()
                }
            }
        }
    }
}

// ——— impl FileOp for Socket ———
// Socket 通过实现 FileOp 接入内核的 VFS 层，可以存入 fd_table，
// 并通过 read / write / poll 等标准接口操作。

impl FileOp for Socket {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.recv(buf),
            SocketInner::Udp(udp) => {
                let (len, _addr) = udp.recv_from(buf)?;
                Ok(len)
            }
            SocketInner::Unix(unix) => unix.read(buf),
        }
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.send(buf),
            SocketInner::Udp(udp) => udp.send(buf),
            SocketInner::Unix(unix) => unix.write(buf),
        }
    }

    /// 非阻塞可读：poll 网络接口后检查 socket 是否有数据。
    fn read_ready(&self) -> bool {
        self.tcp_poll(true)
    }

    /// 非阻塞可写：poll 网络接口后检查 socket 是否可写。
    fn write_ready(&self) -> bool {
        self.tcp_poll(false)
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    /// 套接字不支持 seek。
    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn get_flags(&self) -> OpenFlags {
        let mut flags = OpenFlags::O_RDWR;
        if self.is_nonblocking() {
            flags |= OpenFlags::O_NONBLOCK;
        }
        if self.cloexec.load(Ordering::Acquire) {
            flags |= OpenFlags::O_CLOEXEC;
        }
        flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Socket))
    }

    fn fsync(&self) -> SysResult<usize> {
        Ok(0)
    }
}

// ——— 辅助函数 ———

/// 将系统调用中的 domain 参数解析为 `SocketDomain`。
pub fn parse_domain(domain: usize) -> Result<SocketDomain, Errno> {
    match domain {
        1 => Ok(SocketDomain::AF_UNIX),
        2 => Ok(SocketDomain::AF_INET),
        10 => Ok(SocketDomain::AF_INET6),
        _ => Err(Errno::EAFNOSUPPORT),
    }
}

/// 将系统调用中的 type 参数解析为 `SocketKind`。
pub fn parse_kind(kind: usize) -> Result<SocketKind, Errno> {
    match kind & 0xFF {
        1 => Ok(SocketKind::SOCK_STREAM),
        2 => Ok(SocketKind::SOCK_DGRAM),
        3 => Ok(SocketKind::SOCK_RAW),
        _ => Err(Errno::EINVAL),
    }
}

/// `socket()` 系统调用的 type 参数中的 SOCK_NONBLOCK 标志位。
pub const SOCK_NONBLOCK: usize = 0x800;
/// `socket()` 系统调用的 type 参数中的 SOCK_CLOEXEC 标志位。
pub const SOCK_CLOEXEC: usize = 0x80000;
