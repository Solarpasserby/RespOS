// os/src/drivers/virtio/net_dev.rs

//! VirtIO network device driver.
//!
//! Wraps `virtio_drivers::device::net::VirtIONet` behind a mutex so the single
//! device can be shared between the smoltcp receive/transmit tokens. RX buffers
//! are pre-allocated by `VirtIONet::new` and recycled through
//! [`VirtIoNetDev::recycle_rx_buffer`].

use spin::Mutex;
use virtio_drivers::{
    Hal,
    device::net::{RxBuffer, TxBuffer, VirtIONet},
    transport::Transport,
};

use crate::drivers::{DevError, DevResult};

/// Number of receive buffers pre-allocated by the driver (virtqueue depth).
const VIRTIO_NET_QUEUE_SIZE: usize = 64;
/// Size of each RX buffer. Must be at least the virtio-net minimum (1526) and
/// large enough for a 1514-byte Ethernet frame plus the 10-byte virtio header.
const VIRTIO_NET_BUF_LEN: usize = 2048;

pub struct VirtIoNetDev<H: Hal, T: Transport> {
    inner: Mutex<VirtIONet<H, T, VIRTIO_NET_QUEUE_SIZE>>,
}

unsafe impl<H: Hal, T: Transport> Send for VirtIoNetDev<H, T> {}
unsafe impl<H: Hal, T: Transport> Sync for VirtIoNetDev<H, T> {}

impl<H: Hal, T: Transport> VirtIoNetDev<H, T> {
    pub fn new(transport: T) -> DevResult<Self> {
        let inner = VirtIONet::<H, T, VIRTIO_NET_QUEUE_SIZE>::new(transport, VIRTIO_NET_BUF_LEN)
            .map_err(as_dev_err)?;
        Ok(Self {
            inner: Mutex::new(inner),
        })
    }

    /// The device-provided MAC address.
    pub fn mac_address(&self) -> [u8; 6] {
        self.inner.lock().mac_address()
    }

    /// Whether at least one receive buffer has completed.
    pub fn can_recv(&self) -> bool {
        self.inner.lock().can_recv()
    }

    /// Pop one completed RX buffer, or `DevError::Again` when empty.
    pub fn receive(&self) -> DevResult<RxBuffer> {
        self.inner.lock().receive().map_err(as_dev_err)
    }

    /// Allocate a new TX buffer for an Ethernet frame.
    pub fn new_tx_buffer(&self, len: usize) -> TxBuffer {
        self.inner.lock().new_tx_buffer(len)
    }

    /// Return an RX buffer to the driver so it can be reused.
    pub fn recycle_rx_buffer(&self, rx_buf: RxBuffer) -> DevResult {
        self.inner
            .lock()
            .recycle_rx_buffer(rx_buf)
            .map_err(as_dev_err)
    }

    /// Transmit an Ethernet frame. `TxBuffer` owns the frame bytes; the driver
    /// prepends the virtio-net header itself.
    pub fn send(&self, tx_buf: TxBuffer) -> DevResult {
        self.inner.lock().send(tx_buf).map_err(as_dev_err)
    }
}

#[allow(dead_code)]
const fn as_dev_err(e: virtio_drivers::Error) -> DevError {
    use virtio_drivers::Error::*;
    match e {
        NotReady => DevError::Again,
        AlreadyUsed => DevError::AlreadyExists,
        InvalidParam => DevError::InvalidParam,
        DmaError => DevError::NoMemory,
        IoError => DevError::Io,
        _ => DevError::BadState,
    }
}
