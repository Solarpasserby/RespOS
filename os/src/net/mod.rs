//! RespOS 网络协议栈。
//!
//! 基于 smoltcp 库实现 TCP/IP 协议族，当前支持：
//! - IPv4 回环通信（127.0.0.1）
//! - TCP 流式套接字（SOCK_STREAM）
//! - UDP 数据报套接字（SOCK_DGRAM）
//!
//! ## 架构
//!
//! ```text
//! syscall/net.rs → net/socket.rs (FileOp) → net/tcp.rs / net/udp.rs
//!                                                   ↓
//!                                          smoltcp SocketSet
//!                                                   ↓
//!                                     smoltcp Interface::poll()
//!                                                   ↓
//!                                     net/loopback.rs (LoopbackDev)
//! ```
//!
//! ## 全局单例
//!
//! - `SOCKET_SET` — 所有 smoltcp socket 的集合
//! - `LOOPBACK_IFACE` / `LOOPBACK_DEV` — 回环接口及设备
//! - `LISTEN_TABLE` — TCP 端口监听表

use alloc::vec;
use lazy_static::lazy_static;
use smoltcp::{
    iface::{Config, Interface, SocketHandle, SocketSet},
    phy::Medium,
    socket::{
        tcp::SocketBuffer,
        udp::PacketBuffer,
        AnySocket,
    },
    storage::PacketMetadata,
    time::Instant as SmolInstant,
    wire::{
        HardwareAddress, IpAddress, IpCidr,
    },
};

use crate::arch::timer::get_time_ms;
use crate::mutex::SpinLock;
use crate::syscall::Errno;

mod addr;
mod listen;
mod loopback;
pub mod socket;
pub mod tcp;
pub mod udp;

pub use addr::{
    from_ipendpoint_to_socketaddr, from_sockaddr_to_ipendpoint, is_unspecified,
    LOOP_BACK_ENDPOINT, LOOP_BACK_IP, UNSPECIFIED_ENDPOINT, UNSPECIFIED_IP,
};
pub use listen::ListenTable;
pub use loopback::LoopbackDev;

/// TCP 接收/发送缓冲区默认大小（64 KiB）。
const TCP_RX_BUF_LEN: usize = 64 * 1024;
const TCP_TX_BUF_LEN: usize = 64 * 1024;
/// UDP 数据报缓冲区的元数据槽数和总字节数。
const UDP_RX_BUF_LEN: usize = 64 * 1024;
const UDP_TX_BUF_LEN: usize = 64 * 1024;
// ——— 全局单例 ———

lazy_static! {
    /// 全局 smoltcp socket 集合，所有 TCP/UDP socket 均注册在此。
    static ref SOCKET_SET_INNER: SpinLock<SocketSetWrapper<'static>> =
        SpinLock::new(SocketSetWrapper::new());
    /// TCP 端口监听表，65536 个端口，每个端口维护一个 SYN 队列。
    pub static ref LISTEN_TABLE: SpinLock<ListenTable> =
        SpinLock::new(ListenTable::new());
}

lazy_static! {
    /// 回环设备实例。
    static ref LOOPBACK_DEV: SpinLock<LoopbackDev> =
        SpinLock::new(LoopbackDev::new(Medium::Ip));
    /// 回环网络接口（smoltcp Interface）。
    static ref LOOPBACK_IFACE: SpinLock<Interface> =
        SpinLock::new(create_loopback_iface());
}

/// 对外暴露给 listen.rs / tcp.rs / udp.rs 使用。
pub(crate) fn socket_set() -> &'static SpinLock<SocketSetWrapper<'static>> {
    &SOCKET_SET_INNER
}

/// 创建并配置回环接口（127.0.0.1/8）。
fn create_loopback_iface() -> Interface {
    let mut dev = LoopbackDev::new(Medium::Ip);
    let config = Config::new(HardwareAddress::Ip);
    let timestamp = SmolInstant::from_micros((get_time_ms() * 1000) as i64);
    let mut iface = Interface::new(config, &mut dev, timestamp);
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8))
            .unwrap();
    });
    iface
}

/// 初始化网络栈（在 `mm::init()` 之后调用）。
///
/// lazy_static 变量在首次访问时自动初始化，这里 force touch 确保它们在启动阶段被初始化。
pub fn init() {
    let _ = &*SOCKET_SET_INNER;
    let _ = &*LISTEN_TABLE;
    let _ = &*LOOPBACK_DEV;
    let _ = &*LOOPBACK_IFACE;
}

// ——— SocketSetWrapper ———

/// 全局 socket 集合的线程安全包装。
///
/// 封装 smoltcp 的 `SocketSet`，提供带锁的访问方法。
pub struct SocketSetWrapper<'a>(SpinLock<SocketSet<'a>>);

impl<'a> SocketSetWrapper<'a> {
    fn new() -> Self {
        SocketSetWrapper(SpinLock::new(SocketSet::new(vec![])))
    }

    /// 获取 socket 的只读引用并执行闭包。
    pub fn with_socket<F, T: AnySocket<'a>, R>(&self, handle: SocketHandle, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let binding = self.0.lock();
        let socket = binding.get(handle);
        f(socket)
    }

    /// 获取 socket 的可变引用并执行闭包。
    pub fn with_socket_mut<F, T: AnySocket<'a>, R>(&self, handle: SocketHandle, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut binding = self.0.lock();
        let socket = binding.get_mut(handle);
        f(socket)
    }

    /// 创建一个配置好缓冲区的 smoltcp TCP socket。
    pub fn new_tcp_socket() -> smoltcp::socket::tcp::Socket<'a> {
        let tcp_recv_buffer = SocketBuffer::new(vec![0; TCP_RX_BUF_LEN]);
        let tcp_send_buffer = SocketBuffer::new(vec![0; TCP_TX_BUF_LEN]);
        smoltcp::socket::tcp::Socket::new(tcp_recv_buffer, tcp_send_buffer)
    }

    /// 创建一个配置好缓冲区的 smoltcp UDP socket。
    pub fn new_udp_socket() -> smoltcp::socket::udp::Socket<'a> {
        let udp_recv_buffer =
            PacketBuffer::new(vec![PacketMetadata::EMPTY; 256], vec![0; UDP_RX_BUF_LEN]);
        let udp_send_buffer =
            PacketBuffer::new(vec![PacketMetadata::EMPTY; 256], vec![0; UDP_TX_BUF_LEN]);
        smoltcp::socket::udp::Socket::new(udp_recv_buffer, udp_send_buffer)
    }

    /// 将 socket 注册到集合中，返回句柄。
    pub fn add<T: AnySocket<'a>>(&self, socket: T) -> SocketHandle {
        self.0.lock().add(socket)
    }

    /// 从集合中移除并销毁 socket。
    pub fn remove(&self, handle: SocketHandle) {
        let socket = self.0.lock().remove(handle);
        drop(socket);
    }

    /// 驱动回环接口的收发：调用 smoltcp `Interface::poll()`。
    pub fn poll_interfaces(&self) {
        let timestamp = SmolInstant::from_micros((get_time_ms() * 1000) as i64);
        let mut iface = LOOPBACK_IFACE.lock();
        let mut dev = LOOPBACK_DEV.lock();
        let mut sockets = self.0.lock();
        iface.poll(timestamp, &mut *dev, &mut sockets);
    }

    /// 检查指定地址:端口是否已被占用（用于 bind 前的冲突检测）。
    pub fn bind_check(&self, addr: IpAddress, port: u16) -> Result<usize, Errno> {
        use smoltcp::socket::Socket;
        let mut sockets = self.0.lock();
        for item in sockets.iter_mut() {
            match item.1 {
                Socket::Tcp(socket) => {
                    if socket.local_endpoint().is_some_and(|endpoint| endpoint.addr == addr && endpoint.port == port) {
                        return Err(Errno::EADDRINUSE);
                    }
                }
                Socket::Udp(socket) => {
                    if socket.endpoint().addr == Some(addr) && socket.endpoint().port == port {
                        return Err(Errno::EADDRINUSE);
                    }
                }
            };
        }
        Ok(0)
    }
}

// ——— 公共入口 ———

/// 驱动网络接口的收发操作。
///
/// 在 `block_on` 循环中被频繁调用，确保 smoltcp 状态机持续前进。
pub fn poll_interfaces() {
    SOCKET_SET_INNER.lock().poll_interfaces();
}
