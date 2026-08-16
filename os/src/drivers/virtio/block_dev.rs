// os/src/drivers/virtio/block_dev.rs

use crate::config::BLOCK_SIZE;
use crate::drivers::{BlockDevice, DevError, DevResult, Device, DeviceType};
use spin::Mutex;
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::Transport,
    // transport::mmio::VirtIOHeader
    Hal,
};

pub struct VirtIoBlkDev<H: Hal, T: Transport> {
    inner: Mutex<VirtIOBlk<H, T>>,
}

unsafe impl<H: Hal, T: Transport> Send for VirtIoBlkDev<H, T> {}
unsafe impl<H: Hal, T: Transport> Sync for VirtIoBlkDev<H, T> {}

impl<H: Hal, T: Transport> VirtIoBlkDev<H, T> {
    pub fn new(header: T) -> DevResult<Self> {
        Ok(Self {
            inner: Mutex::new(VirtIOBlk::<H, T>::new(header).map_err(as_dev_err)?),
        })
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
        let started = crate::perf::now_ticks();
        let result = self
            .inner
            .lock()
            .read_blocks(block_id as _, buf)
            .map_err(as_dev_err);
        if result.is_ok() {
            crate::perf::block_read_ticks(crate::perf::elapsed_since(started));
            crate::perf::block_read_request(1);
            crate::perf::block_read_bytes(buf.len());
            crate::perf::block_read_size(buf.len());
            if let Some(task) = crate::task::current_task() {
                task.note_input_blocks(buf.len() / BLOCK_SIZE);
            }
        } else if let Err(error) = &result {
            println!(
                "[virtio-blk-error] op=read block={} bytes={} error={:?}",
                block_id,
                buf.len(),
                error
            );
        }
        result
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> DevResult {
        let started = crate::perf::now_ticks();
        let result = self
            .inner
            .lock()
            .write_blocks(block_id as _, buf)
            .map_err(as_dev_err);
        if result.is_ok() {
            crate::perf::block_write_ticks(crate::perf::elapsed_since(started));
            crate::perf::block_write_request(1);
            crate::perf::block_write_bytes(buf.len());
        } else if let Err(error) = &result {
            println!(
                "[virtio-blk-error] op=write block={} bytes={} error={:?}",
                block_id,
                buf.len(),
                error
            );
        }
        result
    }

    fn flush(&self) -> DevResult {
        let result = self.inner.lock().flush().map_err(as_dev_err);
        if result.is_ok() {
            crate::perf::block_flush(1);
        } else if let Err(error) = &result {
            println!("[virtio-blk-error] op=flush error={:?}", error);
        }
        result
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
