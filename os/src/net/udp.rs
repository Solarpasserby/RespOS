//! UDP 套接字实现。
//!
//! UDP 是无连接协议，没有 TCP 那样的状态机。直接通过 smoltcp 的
//! `udp::Socket` 进行数据报的收发。阻塞语义同样使用 `block_on` 模式。

use alloc::vec;
use core::{
    cell::UnsafeCell,
    net::SocketAddr,
    sync::atomic::{AtomicBool, Ordering},
};
use smoltcp::{
    iface::SocketHandle,
    socket::udp,
    wire::{IpEndpoint, IpListenEndpoint},
};
use spin::{Mutex, RwLock};

use crate::{
    net::addr::{
        LOOP_BACK_IP, from_ipendpoint_to_socketaddr, from_sockaddr_to_ipendpoint, is_unspecified,
    },
    syscall::{Errno, SysResult},
    task::yield_current_task,
};

use super::tcp::PollState;
use super::{SocketSetWrapper, poll_interfaces, socket_set};

/// UDP 套接字。
///
/// 维护 smoltcp socket 句柄、本地/远程地址、以及非阻塞/端口复用标志。
pub struct UdpSocket {
    /// smoltcp socket 句柄。
    handle: UnsafeCell<Option<SocketHandle>>,
    /// bind 的本地地址。
    local_addr: RwLock<Option<IpEndpoint>>,
    /// connect 设置的远程地址。
    remote_addr: RwLock<Option<IpEndpoint>>,
    /// 是否非阻塞模式。
    nonblock: AtomicBool,
    /// 是否允许端口复用（SO_REUSEADDR）。
    reuse_addr: AtomicBool,
}

// SAFETY: 单核协作式调度，字段访问在 block_on 内串行化。
unsafe impl Sync for UdpSocket {}
unsafe impl Send for UdpSocket {}

impl UdpSocket {
    /// 创建一个新的 UDP 套接字并在全局 socket 集合中注册。
    pub fn new() -> Self {
        let udp_socket = SocketSetWrapper::new_udp_socket();
        let handle = socket_set().lock().add(udp_socket);
        UdpSocket {
            handle: UnsafeCell::new(Some(handle)),
            local_addr: RwLock::new(None),
            remote_addr: RwLock::new(None),
            nonblock: AtomicBool::new(false),
            reuse_addr: AtomicBool::new(false),
        }
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Errno> {
        match self.local_addr.try_read() {
            Some(addr) => addr
                .map(from_ipendpoint_to_socketaddr)
                .ok_or(Errno::ENOTCONN),
            None => Err(Errno::ENOTCONN),
        }
    }

    pub fn remote_addr(&self) -> Result<SocketAddr, Errno> {
        match self.remote_addr.try_read() {
            Some(addr) => addr
                .map(from_ipendpoint_to_socketaddr)
                .ok_or(Errno::ENOTCONN),
            None => Err(Errno::ENOTCONN),
        }
    }

    pub fn is_nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    pub fn set_nonblocking(&self, block: bool) {
        self.nonblock.store(block, Ordering::Release);
    }

    /// 设置 TTL / hop limit。
    pub fn set_socket_ttl(&self, ttl: u8) {
        let handle = unsafe { self.handle.get().read().unwrap() };
        socket_set()
            .lock()
            .with_socket_mut::<_, udp::Socket, _>(handle, |socket| {
                socket.set_hop_limit(Some(ttl));
            });
    }

    pub fn is_reuse_addr(&self) -> bool {
        self.reuse_addr.load(Ordering::Acquire)
    }

    pub fn set_reuse_addr(&self, reuse: bool) {
        self.reuse_addr.store(reuse, Ordering::Release);
    }

    pub fn is_block(&self) -> bool {
        !self.is_nonblocking()
    }

    /// 绑定本地地址和端口。若地址未指定则默认使用回环地址。
    pub fn bind(&self, mut bind_addr: SocketAddr) -> SysResult {
        let mut local_addr = self.local_addr.write();
        if local_addr.is_some() {
            return Err(Errno::EINVAL);
        }
        if bind_addr.port() == 0 {
            bind_addr.set_port(get_ephemeral_port());
        }
        let mut local_endpoint = from_sockaddr_to_ipendpoint(bind_addr);
        if is_unspecified(local_endpoint.addr) {
            local_endpoint.addr = LOOP_BACK_IP;
        }
        let endpoint = IpListenEndpoint {
            addr: (!is_unspecified(local_endpoint.addr)).then_some(local_endpoint.addr),
            port: local_endpoint.port,
        };
        if !self.is_reuse_addr() {
            socket_set()
                .lock()
                .udp_bind_check(local_endpoint.addr, local_endpoint.port)?;
        }
        let handle = unsafe { self.handle.get().read().unwrap() };
        socket_set()
            .lock()
            .with_socket_mut::<_, udp::Socket, Result<(), Errno>>(handle, |socket| {
                socket.bind(endpoint).map_err(|_| Errno::EADDRINUSE)?;
                Ok(())
            })?;
        *local_addr = Some(local_endpoint);
        Ok(())
    }

    /// 向指定地址发送数据报。
    pub fn send_to(&self, buf: &[u8], remote_addr: SocketAddr) -> Result<usize, Errno> {
        self.send_impl(buf, from_sockaddr_to_ipendpoint(remote_addr))
    }

    /// 接收数据报，同时返回发送方地址。
    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), Errno> {
        let mut binding = vec![0; 1528];
        let kernel_buf = binding.as_mut_slice();

        self.recv_impl(|socket| match socket.recv_slice(kernel_buf) {
            Ok((len, meta)) => {
                let copy_len = core::cmp::min(len, buf.len());
                buf[..copy_len].copy_from_slice(&kernel_buf[..copy_len]);
                Ok((copy_len, from_ipendpoint_to_socketaddr(meta.endpoint)))
            }
            Err(e) => match e {
                udp::RecvError::Exhausted => Err(Errno::EAGAIN),
                udp::RecvError::Truncated => Err(Errno::EAGAIN),
            },
        })
    }

    /// 设置默认远程地址（connect 后可直接用 send/recv）。
    pub fn connect(&self, remote_addr: SocketAddr) -> Result<(), Errno> {
        let mut self_remote_addr = self.remote_addr.write();
        if self.local_addr.read().is_none() {
            self.bind(from_ipendpoint_to_socketaddr(IpEndpoint::new(
                LOOP_BACK_IP,
                get_ephemeral_port(),
            )))?;
        }
        let remote_endpoint = from_sockaddr_to_ipendpoint(remote_addr);
        let handle = unsafe { self.handle.get().read().unwrap() };
        socket_set()
            .lock()
            .with_socket_mut::<_, udp::Socket, _>(handle, |socket| {
                socket.connect(remote_endpoint);
            });
        *self_remote_addr = Some(remote_endpoint);
        Ok(())
    }

    /// 向 connect 时设置的远程地址发送数据。
    pub fn send(&self, buf: &[u8]) -> Result<usize, Errno> {
        let remote_endpoint = *self
            .remote_addr
            .read()
            .as_ref()
            .ok_or(Errno::EDESTADDRREQ)?;
        self.send_impl(buf, remote_endpoint)
    }

    /// 从 connect 时设置的远程地址接收数据。
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let remote_endpoint = *self.remote_addr.read().as_ref().ok_or(Errno::ENOTCONN)?;
        self.recv_impl(|socket| {
            let (_, meta) = socket.peek_slice(&mut []).map_err(|e| match e {
                udp::RecvError::Exhausted => Errno::EAGAIN,
                udp::RecvError::Truncated => Errno::EAGAIN,
            })?;
            if !is_unspecified(remote_endpoint.addr) && remote_endpoint.addr != meta.endpoint.addr {
                return Err(Errno::EAGAIN);
            }
            if remote_endpoint.port != 0 && remote_endpoint.port != meta.endpoint.port {
                return Err(Errno::EAGAIN);
            }
            let (len, _) = socket.recv_slice(buf).map_err(|e| match e {
                udp::RecvError::Exhausted => Errno::EAGAIN,
                udp::RecvError::Truncated => Errno::EMSGSIZE,
            })?;
            Ok(len)
        })
    }

    /// 关闭 socket。
    pub fn shutdown(&self) {
        let handle = unsafe { self.handle.get().read().unwrap() };
        socket_set()
            .lock()
            .with_socket_mut::<_, udp::Socket, _>(handle, |socket| {
                socket.close();
            });
    }

    /// 查询可读/可写状态。
    pub fn poll(&self) -> PollState {
        if self.local_addr.read().is_none() {
            return PollState {
                readable: false,
                writeable: false,
            };
        }
        let handle = unsafe { self.handle.get().read().unwrap() };
        socket_set()
            .lock()
            .with_socket_mut::<_, udp::Socket, _>(handle, |socket| PollState {
                readable: socket.can_recv(),
                writeable: socket.can_send(),
            })
    }
}

// ——— 私有方法 ———
impl UdpSocket {
    /// 接收实现：block_on 循环中尝试接收。
    fn recv_impl<F, T>(&self, mut op: F) -> Result<T, Errno>
    where
        F: FnMut(&mut udp::Socket) -> Result<T, Errno>,
    {
        if self.local_addr.read().is_none() {
            return Err(Errno::ENOTCONN);
        }
        self.block_on(|| {
            let handle = unsafe { self.handle.get().read().unwrap() };
            socket_set()
                .lock()
                .with_socket_mut::<_, udp::Socket, _>(handle, |socket| {
                    if !socket.is_open() {
                        Err(Errno::ENOTCONN)
                    } else if socket.can_recv() {
                        op(socket)
                    } else {
                        Err(Errno::EAGAIN)
                    }
                })
        })
    }

    /// 发送实现：block_on 循环中尝试发送。
    fn send_impl(&self, buf: &[u8], remote_addr: IpEndpoint) -> Result<usize, Errno> {
        if self.local_addr.read().is_none() {
            self.bind(from_ipendpoint_to_socketaddr(IpEndpoint::new(
                LOOP_BACK_IP,
                get_ephemeral_port(),
            )))?;
        }
        self.block_on(|| {
            let handle = unsafe { self.handle.get().read().unwrap() };
            socket_set()
                .lock()
                .with_socket_mut::<_, udp::Socket, _>(handle, |socket| {
                    if !socket.is_open() {
                        Err(Errno::ENOTCONN)
                    } else if socket.can_send() {
                        socket.send_slice(buf, remote_addr).map_err(|e| match e {
                            udp::SendError::Unaddressable => Errno::EADDRNOTAVAIL,
                            udp::SendError::BufferFull => Errno::EMSGSIZE,
                        })?;
                        Ok(buf.len())
                    } else {
                        Err(Errno::EAGAIN)
                    }
                })
        })
    }

    /// 阻塞循环：poll → 操作 → EAGAIN 则 yield。
    fn block_on<F, T>(&self, mut f: F) -> Result<T, Errno>
    where
        F: FnMut() -> Result<T, Errno>,
    {
        if self.is_nonblocking() {
            f()
        } else {
            loop {
                poll_interfaces();
                match f() {
                    Ok(res) => return Ok(res),
                    Err(e) => {
                        if e == Errno::EAGAIN {
                            yield_current_task();
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
        }
    }
}

/// 分配 UDP 临时端口（范围 0xc000–0xffff）。
pub fn get_ephemeral_port() -> u16 {
    const PORT_START: u16 = 0xc000;
    const PORT_END: u16 = 0xffff;
    static CURR: Mutex<u16> = Mutex::new(PORT_START);
    let mut curr = CURR.lock();
    let mut tries = 0;
    while tries <= PORT_END - PORT_START {
        let port = *curr;
        if *curr == PORT_END {
            *curr = PORT_START;
        } else {
            *curr += 1;
        }
        if socket_set()
            .lock()
            .udp_bind_check(LOOP_BACK_IP, port)
            .is_ok()
        {
            return port;
        }
        tries += 1;
    }
    panic!("no available UDP port");
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        self.shutdown();
        let handle = unsafe { self.handle.get().read().unwrap() };
        socket_set().lock().remove(handle);
    }
}
