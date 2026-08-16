//! smoltcp Ethernet device backed by the virtio-net driver.
//!
//! Adapts [`crate::drivers::NetDeviceImpl`] to smoltcp's [`Device`] trait. RX
//! buffers returned by virtio-net are handed to smoltcp through an
//! [`RxToken`]; the buffer is recycled back to the device when the token is
//! consumed or dropped.

use alloc::sync::Arc;
use smoltcp::phy::{
    Device, DeviceCapabilities, Medium, RxToken as SmolRxToken, TxToken as SmolTxToken,
};
use smoltcp::time::Instant;
use virtio_drivers::device::net::RxBuffer;

use crate::drivers::NetDeviceImpl;

const ETHERNET_MTU: usize = 1500;

/// The smoltcp-facing Ethernet device.
pub struct EthernetDevice {
    dev: Arc<NetDeviceImpl>,
}

impl EthernetDevice {
    pub fn new(dev: Arc<NetDeviceImpl>) -> Self {
        Self { dev }
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.dev.mac_address()
    }
}

/// Receive token. Owns one virtio-net RX buffer until smoltcp consumes it, then
/// recycles it.
pub struct VirtioRxToken {
    buffer: Option<RxBuffer>,
    dev: Arc<NetDeviceImpl>,
}

impl VirtioRxToken {
    fn new(buffer: RxBuffer, dev: Arc<NetDeviceImpl>) -> Self {
        Self {
            buffer: Some(buffer),
            dev,
        }
    }
}

impl SmolRxToken for VirtioRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = self.buffer.take().expect("virtio RX token consumed twice");
        let result = f(buffer.packet_mut());
        let _ = self.dev.recycle_rx_buffer(buffer);
        result
    }
}

impl Drop for VirtioRxToken {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            let _ = self.dev.recycle_rx_buffer(buffer);
        }
    }
}

/// Transmit token. Allocates a fresh virtio-net TX buffer for the frame.
pub struct VirtioTxToken {
    dev: Arc<NetDeviceImpl>,
}

impl SmolTxToken for VirtioTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut tx_buf = self.dev.new_tx_buffer(len);
        let result = f(tx_buf.packet_mut());
        if let Err(error) = self.dev.send(tx_buf) {
            println!("[virtio-net-error] transmit failed: {:?}", error);
        }
        result
    }
}

impl Device for EthernetDevice {
    type RxToken<'a> = VirtioRxToken;
    type TxToken<'a> = VirtioTxToken;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut cap = DeviceCapabilities::default();
        cap.max_transmission_unit = ETHERNET_MTU;
        cap.medium = Medium::Ethernet;
        cap
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self.dev.receive() {
            Ok(buffer) => Some((
                VirtioRxToken::new(buffer, self.dev.clone()),
                VirtioTxToken {
                    dev: self.dev.clone(),
                },
            )),
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(VirtioTxToken {
            dev: self.dev.clone(),
        })
    }
}
