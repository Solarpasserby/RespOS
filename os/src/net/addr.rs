//! 网络地址转换工具。
//!
//! 在标准库 `core::net::SocketAddr` 与 smoltcp `wire::IpEndpoint` 之间进行转换，
//! 同时提供特殊的回环地址和未指定地址常量。

use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address, Ipv6Address};

/// 未指定 IP 地址（0.0.0.0）。
pub const UNSPECIFIED_IP: IpAddress = IpAddress::v4(0, 0, 0, 0);
/// 回环 IP 地址（127.0.0.1）。
pub const LOOP_BACK_IP: IpAddress = IpAddress::v4(127, 0, 0, 1);
/// 回环端点（127.0.0.1:11111）。
pub const LOOP_BACK_ENDPOINT: IpEndpoint = IpEndpoint::new(LOOP_BACK_IP, 11111);
/// 未指定端点（0.0.0.0:0）。
pub const UNSPECIFIED_ENDPOINT: IpEndpoint = IpEndpoint::new(UNSPECIFIED_IP, 0);

/// 判断 IP 地址是否为未指定地址（全零）。
pub fn is_unspecified(ip: IpAddress) -> bool {
    ip.as_bytes() == [0, 0, 0, 0]
        || ip.as_bytes() == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

/// 将标准库 `SocketAddr` 转换为 smoltcp `IpEndpoint`。
pub fn from_sockaddr_to_ipendpoint(addr: SocketAddr) -> IpEndpoint {
    let ip = match addr.ip() {
        core::net::IpAddr::V4(ipv4_addr) => IpAddress::Ipv4(Ipv4Address(ipv4_addr.octets())),
        core::net::IpAddr::V6(ipv6_addr) => IpAddress::Ipv6(Ipv6Address(ipv6_addr.octets())),
    };
    IpEndpoint {
        addr: ip,
        port: addr.port(),
    }
}

/// 将 smoltcp `IpEndpoint` 转换为标准库 `SocketAddr`。
pub fn from_ipendpoint_to_socketaddr(addr: IpEndpoint) -> SocketAddr {
    let port = addr.port;
    match addr.addr {
        IpAddress::Ipv4(ipv4) => {
            let octets = ipv4.0;
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(octets), port))
        }
        IpAddress::Ipv6(ipv6) => {
            let segments = ipv6.0;
            let ipv6_addr = Ipv6Addr::from(segments);
            SocketAddr::V6(SocketAddrV6::new(ipv6_addr, port, 0, 0))
        }
    }
}
