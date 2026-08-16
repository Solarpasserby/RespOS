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

use alloc::{collections::BTreeSet, string::String, sync::Arc, vec};
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;
use smoltcp::{
    iface::{Config, Interface, SocketHandle, SocketSet},
    phy::Medium,
    socket::tcp::State as TcpState,
    socket::{AnySocket, tcp::SocketBuffer, udp::PacketBuffer},
    storage::PacketMetadata,
    time::Instant as SmolInstant,
    wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint},
};

use crate::arch::timer::get_time_ms;
use crate::drivers::NetDeviceImpl;
use crate::mutex::SpinLock;
use crate::syscall::Errno;

mod addr;
mod ethernet;
mod http;
mod listen;
mod loopback;
pub mod socket;
pub mod tcp;
pub mod udp;

use ethernet::EthernetDevice;
use http::HttpServer;

/// Guest IP address used by QEMU user (slirp) networking.
pub const GUEST_IP_OCTETS: [u8; 4] = [10, 0, 2, 15];
/// Guest network prefix length (QEMU user network default).
const GUEST_IP_PREFIX: u8 = 24;

pub use addr::{
    LOOP_BACK_ENDPOINT, LOOP_BACK_IP, UNSPECIFIED_ENDPOINT, UNSPECIFIED_IP,
    from_ipendpoint_to_socketaddr, from_sockaddr_to_ipendpoint, is_unspecified,
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
    /// Tasks sleeping in a TCP operation. Protocol polling wakes these tasks;
    /// each task rechecks its own socket condition after it is scheduled.
    static ref TCP_WAITERS: SpinLock<BTreeSet<usize>> = SpinLock::new(BTreeSet::new());
}

lazy_static! {
    /// 回环设备实例。
    static ref LOOPBACK_DEV: SpinLock<LoopbackDev> =
        SpinLock::new(LoopbackDev::new(Medium::Ip));
    /// 回环网络接口（smoltcp Interface）。
    static ref LOOPBACK_IFACE: SpinLock<Interface> =
        SpinLock::new(create_loopback_iface());
}

lazy_static! {
    /// 真实网卡设备（virtio-net），不存在时为 `None`。
    static ref ETH_DEV: SpinLock<Option<EthernetDevice>> = SpinLock::new(None);
    /// 真实网络接口（smoltcp Interface），不存在时为 `None`。
    static ref ETH_IFACE: SpinLock<Option<Interface>> = SpinLock::new(None);
    /// In-kernel HTTP 服务器，仅在网卡存在时初始化。
    static ref HTTP_SERVER: SpinLock<Option<HttpServer>> = SpinLock::new(None);
}

/// Guest IP address as a dotted-quad string.
pub fn guest_ip() -> &'static str {
    "10.0.2.15"
}

/// The MAC address of the virtio-net device, if present.
pub fn eth_mac() -> Option<[u8; 6]> {
    ETH_DEV.lock().as_ref().map(|dev| dev.mac_address())
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

/// 创建并配置真实网络接口（guest ip = 10.0.2.15，QEMU user networking）。
fn create_ethernet_iface(dev: &mut EthernetDevice, mac: [u8; 6]) -> Interface {
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress::from_bytes(
        &mac,
    )));
    let timestamp = SmolInstant::from_micros((get_time_ms() * 1000) as i64);
    let mut iface = Interface::new(config, dev, timestamp);
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(
                IpAddress::v4(
                    GUEST_IP_OCTETS[0],
                    GUEST_IP_OCTETS[1],
                    GUEST_IP_OCTETS[2],
                    GUEST_IP_OCTETS[3],
                ),
                GUEST_IP_PREFIX,
            ))
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
    init_ethernet();
}

/// Try to bring up the virtio-net device, the Ethernet smoltcp interface, and
/// the in-kernel HTTP server. Missing hardware is non-fatal: the loopback-only
/// stack keeps working unchanged.
fn init_ethernet() {
    let device = match NetDeviceImpl::new_device() {
        Ok(device) => device,
        Err(_) => {
            println!("[net] virtio-net not present; real network + HTTP server disabled");
            return;
        }
    };
    let mac = device.mac_address();
    let mut dev = EthernetDevice::new(Arc::new(device));
    let iface = create_ethernet_iface(&mut dev, mac);

    *ETH_DEV.lock() = Some(dev);
    *ETH_IFACE.lock() = Some(iface);
    *HTTP_SERVER.lock() = HttpServer::new();

    println!("[net] virtio-net up, mac={:?}, guest ip={}", mac, guest_ip());
    if HTTP_SERVER.lock().is_none() {
        println!("[net] in-kernel HTTP server failed to bind port {}", http::HTTP_PORT);
    } else {
        println!("[net] in-kernel HTTP server listening on port {}", http::HTTP_PORT);
    }
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

    /// 驱动回环与真实网卡接口的收发：调用 smoltcp `Interface::poll()`。
    ///
    /// The same socket set is polled by both interfaces so a single listener
    /// (e.g. the HTTP server on 0.0.0.0:80) can serve loopback and Ethernet
    /// traffic alike. The Ethernet interface is polled first so a socket that
    /// belongs to a non-loopback address drains its transmit buffer before the
    /// loopback interface (which has no route for it) is consulted.
    pub fn poll_interfaces(&self) {
        let timestamp = SmolInstant::from_micros((get_time_ms() * 1000) as i64);
        let mut iface = LOOPBACK_IFACE.lock();
        let mut dev = LOOPBACK_DEV.lock();
        let mut eth_iface = ETH_IFACE.lock();
        let mut eth_dev = ETH_DEV.lock();
        let mut sockets = self.0.lock();
        if let (Some(eth), Some(ethdev)) = (eth_iface.as_mut(), eth_dev.as_mut()) {
            eth.poll(timestamp, &mut *ethdev, &mut sockets);
        }
        iface.poll(timestamp, &mut *dev, &mut sockets);
    }

    /// 检查指定 TCP 地址:端口是否已被占用（用于 bind/connect 前的冲突检测）。
    pub fn tcp_bind_check(&self, addr: IpAddress, port: u16) -> Result<usize, Errno> {
        use smoltcp::socket::Socket;
        let mut sockets = self.0.lock();
        for item in sockets.iter_mut() {
            if let Socket::Tcp(socket) = item.1 {
                if socket
                    .local_endpoint()
                    .is_some_and(|endpoint| endpoint.addr == addr && endpoint.port == port)
                {
                    return Err(Errno::EADDRINUSE);
                }
            }
        }
        Ok(0)
    }

    /// 检查指定 UDP 地址:端口是否已被占用（用于 bind 前的冲突检测）。
    pub fn udp_bind_check(&self, addr: IpAddress, port: u16) -> Result<usize, Errno> {
        use smoltcp::socket::Socket;
        let mut sockets = self.0.lock();
        for item in sockets.iter_mut() {
            if let Socket::Udp(socket) = item.1 {
                if socket.endpoint().addr == Some(addr) && socket.endpoint().port == port {
                    return Err(Errno::EADDRINUSE);
                }
            }
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
    // A smoltcp listening socket becomes the connected socket after one
    // handshake.  Replenish it as part of protocol polling rather than waiting
    // for userspace accept(2), so the listen backlog remains available to
    // concurrent clients.
    LISTEN_TABLE.lock().promote_ready_listeners();
    wake_tcp_waiters();
}

pub(crate) fn register_tcp_waiter(tid: usize) {
    TCP_WAITERS.lock().insert(tid);
}

pub(crate) fn unregister_tcp_waiter(tid: usize) {
    TCP_WAITERS.lock().remove(&tid);
}

fn wake_tcp_waiters() {
    let tids: alloc::vec::Vec<usize> = TCP_WAITERS.lock().iter().copied().collect();
    for tid in tids {
        crate::task::wakeup_task(tid);
    }
}

/// Advance the in-kernel HTTP server state machine.
///
/// Must run after [`poll_interfaces`] so newly received packets and completed
/// handshakes are already reflected in socket states.
pub fn poll_http() {
    if let Some(server) = HTTP_SERVER.lock().as_mut() {
        server.poll();
    }
}

/// Background network driver for the timer-service hart's idle loop.
///
/// Polls both interfaces and the HTTP server, throttled so a busy CPU never
/// spins on the network device. Returns immediately when no Ethernet device is
/// present: the loopback stack is already driven by socket syscalls.
pub fn poll_background() {
    if ETH_IFACE.lock().is_none() {
        return;
    }
    const POLL_INTERVAL_MS: usize = 10;
    static LAST_POLL_MS: AtomicUsize = AtomicUsize::new(0);
    let now = get_time_ms();
    let last = LAST_POLL_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < POLL_INTERVAL_MS {
        return;
    }
    LAST_POLL_MS.store(now, Ordering::Relaxed);
    poll_interfaces();
    poll_http();
}

/// Consume deferred global timer work from a network retry safe point.
///
/// Blocking socket loops can keep the timer-service hart in kernel mode while
/// at least one waiter remains runnable, so neither a user timer trap nor the
/// idle safe point is guaranteed to run. Call this only while holding no
/// socket, task, signal, or timer lock.
pub(crate) fn service_task_timers() {
    crate::syscall::service_task_timers_at_safe_point();
}

pub(crate) struct TcpProcEntry {
    local: IpEndpoint,
    remote: IpEndpoint,
    state: TcpState,
}

impl TcpProcEntry {
    pub(crate) fn new(local: IpEndpoint, remote: IpEndpoint, state: TcpState) -> Self {
        Self {
            local,
            remote,
            state,
        }
    }
}

pub fn proc_net_tcp() -> String {
    poll_interfaces();

    let mut entries = LISTEN_TABLE.lock().proc_entries();
    {
        use smoltcp::socket::Socket;

        let socket_set = SOCKET_SET_INNER.lock();
        let mut sockets = socket_set.0.lock();
        for (_, socket) in sockets.iter_mut() {
            let Socket::Tcp(socket) = socket else {
                continue;
            };
            let Some(local) = socket.local_endpoint() else {
                continue;
            };
            let remote = socket
                .remote_endpoint()
                .unwrap_or(IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), 0));
            entries.push(TcpProcEntry::new(local, remote, socket.state()));
        }
    }

    let mut content = String::from(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
    );
    for (idx, entry) in entries.iter().enumerate() {
        let _ = writeln!(
            content,
            "{:4}: {:08X}:{:04X} {:08X}:{:04X} {:02X} 00000000:00000000 00:00000000 00000000 {:5} {:8} {}",
            idx,
            proc_ipv4_hex(entry.local.addr),
            entry.local.port,
            proc_ipv4_hex(entry.remote.addr),
            entry.remote.port,
            proc_tcp_state(entry.state),
            0,
            0,
            idx + 1,
        );
    }
    content
}

fn proc_ipv4_hex(addr: IpAddress) -> u32 {
    let bytes = addr.as_bytes();
    if bytes.len() == 4 {
        (bytes[0] as u32)
            | ((bytes[1] as u32) << 8)
            | ((bytes[2] as u32) << 16)
            | ((bytes[3] as u32) << 24)
    } else {
        0
    }
}

fn proc_tcp_state(state: TcpState) -> u8 {
    match state {
        TcpState::Established => 0x01,
        TcpState::SynSent => 0x02,
        TcpState::SynReceived => 0x03,
        TcpState::FinWait1 => 0x04,
        TcpState::FinWait2 => 0x05,
        TcpState::TimeWait => 0x06,
        TcpState::Closed => 0x07,
        TcpState::CloseWait => 0x08,
        TcpState::LastAck => 0x09,
        TcpState::Listen => 0x0A,
        TcpState::Closing => 0x0B,
    }
}
