//! 本地回环网络设备。
//!
//! 实现 smoltcp 的 `Device` trait，所有发送的数据包直接推入内部队列，
//! 下一次 poll 时作为接收数据返回，实现 127.0.0.1 的本地通信。

extern crate alloc;

use alloc::{collections::VecDeque, vec, vec::Vec};
use smoltcp::phy::{
    Device, DeviceCapabilities, Medium, RxToken as SmoltcpRx, TxToken as SmoltcpTx,
};
use smoltcp::time::Instant;
use smoltcp::wire::{IpProtocol, Ipv4Packet, TcpPacket};

/// 回环设备，内部维护一个待接收队列和一个空闲缓冲区池。
pub struct LoopbackDev {
    /// 已发送待接收的数据包队列。
    queue: VecDeque<Vec<u8>>,
    /// 空闲缓冲区池，用于复用已分配的缓冲区。
    pool: Vec<Vec<u8>>,
    /// 介质类型（IP 或 Ethernet）。
    medium: Medium,
}

impl LoopbackDev {
    pub fn new(medium: Medium) -> Self {
        Self {
            queue: VecDeque::new(),
            pool: Vec::new(),
            medium,
        }
    }

    /// 从池中取一个长度至少为 `len` 的缓冲区。
    fn take_buf(&mut self, len: usize) -> Vec<u8> {
        if let Some(mut buf) = self.pool.pop() {
            buf.clear();
            buf.resize(len, 0);
            buf
        } else {
            vec![0; len]
        }
    }

    /// 将缓冲区回收到池中（最多缓存 64 个）。
    fn recycle_buf(&mut self, mut buf: Vec<u8>) {
        if self.pool.len() < 64 {
            buf.clear();
            self.pool.push(buf);
        }
    }
}

/// 接收令牌，包含收到的数据包缓冲区及指向设备的指针（用于回收）。
pub struct RxTokenScoop {
    buffer: Vec<u8>,
    dev: *mut LoopbackDev,
}

impl Drop for RxTokenScoop {
    fn drop(&mut self) {
        // 令牌被丢弃时，将缓冲区归还到池中
        unsafe {
            if let Some(dev) = self.dev.as_mut() {
                let buf = core::mem::take(&mut self.buffer);
                dev.recycle_buf(buf);
            }
        }
    }
}

impl SmoltcpRx for RxTokenScoop {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.buffer)
    }
}

/// 发送令牌，持有设备指针以从池中分配缓冲区。
pub struct TxToken {
    dev: *mut LoopbackDev,
}

impl SmoltcpTx for TxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = unsafe { (*self.dev).take_buf(len) };
        let ret = f(&mut buffer);

        // 强制计算 IP 和 TCP 校验和（smoltcp 在回环模式下可能跳过）
        if let Ok(mut ipv4) = Ipv4Packet::new_checked(&mut buffer) {
            ipv4.fill_checksum();
            if ipv4.next_header() == IpProtocol::Tcp {
                if let Ok(mut tcp) = TcpPacket::new_checked(ipv4.payload_mut()) {
                    let csum = tcp.checksum();
                    tcp.set_checksum(csum);
                }
            }
        }

        // 推送到接收队列，下一次 poll 时被取出
        unsafe {
            (*self.dev).queue.push_back(buffer);
        }
        ret
    }
}

impl Device for LoopbackDev {
    type RxToken<'a> = RxTokenScoop;
    type TxToken<'a> = TxToken;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut cap = DeviceCapabilities::default();
        cap.max_transmission_unit = 3_000_000;
        cap.medium = self.medium;
        cap
    }

    /// 从队列中取出一个待接收的数据包。
    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let dev_ptr = self as *mut _;
        self.queue.pop_front().map(|buf| {
            let rx = RxTokenScoop {
                buffer: buf,
                dev: dev_ptr,
            };
            let tx = TxToken { dev: dev_ptr };
            (rx, tx)
        })
    }

    /// 返回发送令牌，数据由 smoltcp 填充后推入队列。
    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken {
            dev: self as *mut _,
        })
    }
}
