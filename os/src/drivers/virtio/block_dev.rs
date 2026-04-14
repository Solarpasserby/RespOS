// os/src/drivers/virtio/block_dev.rs

use spin::Mutex;
use virtio_drivers::{
    Hal, 
    device::blk::VirtIOBlk,
    transport::Transport, 
    // transport::mmio::VirtIOHeader
};
use crate::drivers::{Device, BlockDevice, DevResult, DevError, DeviceType};
use crate::config::BLOCK_SIZE;

pub struct VirtIoBlkDev<H: Hal, T: Transport> {
    inner: Mutex<VirtIOBlk<H, T>>,
}

unsafe impl<H: Hal, T: Transport> Send for VirtIoBlkDev<H, T> {}
unsafe impl<H: Hal, T: Transport> Sync for VirtIoBlkDev<H, T> {}

impl<H: Hal, T: Transport> VirtIoBlkDev<H, T> {
    pub fn new(header: T) -> Self {
        Self {
            inner: Mutex::new(VirtIOBlk::<H, T>::new(header).expect("[kernel] VirtIOBlk create failed")),
        }
    }
}

impl<H: Hal + 'static, T: Transport + 'static> Device for VirtIoBlkDev<H, T> {
    fn device_name(&self) -> &str {
        "virtio-blk"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl<H: Hal + 'static, T: Transport + 'static> BlockDevice for VirtIoBlkDev<H, T> {
    #[inline]
    fn num_blocks(&self) -> usize {
        self.inner.lock().capacity() as usize
    }

    #[inline]
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> DevResult {
        self.inner
            .lock()
            .read_blocks(block_id as _, buf)
            .map_err(as_dev_err)
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> DevResult {
        self.inner
            .lock()
            .write_blocks(block_id as _, buf)
            .map_err(as_dev_err)
    }

    fn flush(&self) -> DevResult {
        self.inner.lock().flush().map_err(as_dev_err)
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
