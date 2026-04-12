/// All supported device types.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DeviceType {
    /// 块设备 (e.g., disk).
    Block,
    /// 字符设备 (e.g., serial port).
    Char,
    /// Network device (e.g., ethernet card).
    // Net,
    /// Graphic display device (e.g., GPU)
    // Display,
    /// 其他
    Unknown,
}

/// 设备操作返回值类型
pub type DevResult<T = ()> = Result<T, DevError>;

/// The error type for device operation failures.
#[derive(Debug)]
pub enum DevError {
    /// An entity already exists.
    AlreadyExists,
    /// Try again, for non-blocking APIs.
    Again,
    /// Bad internal state.
    BadState,
    /// Invalid parameter/argument.
    InvalidParam,
    /// Input/output error.
    Io,
    /// Not enough space/cannot allocate memory (DMA).
    NoMemory,
    /// Device or resource is busy.
    ResourceBusy,
    /// This operation is unsupported or unimplemented.
    Unsupported,
}

pub trait Device: Send + Sync {
    /// 设备名称
    fn device_name(&self) -> &str;

    /// 设备类型
    fn device_type(&self) -> DeviceType;
}

/// Operations that require a block storage device driver to implement.
pub trait BlockDevice: Device {
    /// The number of blocks in this storage device.
    ///
    /// The total size of the device is `num_blocks() * block_size()`.
    fn num_blocks(&self) -> usize;
    /// The size of each block in bytes.
    fn block_size(&self) -> usize;

    /// Reads blocked data from the given block.
    ///
    /// The size of the buffer may exceed the block size, in which case multiple
    /// contiguous blocks will be read.
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> DevResult;

    /// Writes blocked data to the given block.
    ///
    /// The size of the buffer may exceed the block size, in which case multiple
    /// contiguous blocks will be written.
    fn write_block(&self, block_id: usize, buf: &[u8]) -> DevResult;

    /// Flushes the device to write all pending data to the storage.
    fn flush(&self) -> DevResult;
}
