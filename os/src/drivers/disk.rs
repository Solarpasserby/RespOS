// os/src/drivers/disk.rs

use super::{BlockDevice, DevResult};
use crate::config::BLOCK_SIZE;
use alloc::sync::Arc;
use lwext4_rust::KernelDevOp;

/// 块设备读写器——提供对块设备的连续读写
///
/// 使用偏移量，将连续读写转换为块读写
pub struct Disk {
    block_id: usize,
    offset: usize,
    dev: Arc<dyn BlockDevice>,
    /// 分区起始块号（512 字节块）；整盘使用时为 0。
    base_block: usize,
}

impl Disk {
    pub fn new(dev: Arc<dyn BlockDevice>, base_block: usize) -> Self {
        assert_eq!(BLOCK_SIZE, dev.block_size());
        assert!(base_block < dev.num_blocks(), "root disk base block out of range");
        Self {
            block_id: 0,
            offset: 0,
            dev,
            base_block,
        }
    }

    /// 获取分区总数据大小（从 base_block 到设备末尾）。
    pub fn size(&self) -> usize {
        (self.dev.num_blocks() - self.base_block) * BLOCK_SIZE
    }

    /// 获取读写位置
    pub fn position(&self) -> usize {
        self.block_id * BLOCK_SIZE + self.offset
    }

    /// 设置读写位置
    pub fn set_position(&mut self, pos: usize) {
        self.block_id = pos / BLOCK_SIZE;
        self.offset = pos % BLOCK_SIZE;
    }

    /// Read the partial head or tail left around an aligned batched request.
    fn read_partial(&mut self, buf: &mut [u8]) -> DevResult<usize> {
        debug_assert!(self.offset != 0 || buf.len() < BLOCK_SIZE);
        let mut data = [0u8; BLOCK_SIZE];
        let start = self.offset;
        let count = buf.len().min(BLOCK_SIZE - self.offset);

        self.dev
            .read_block(self.base_block + self.block_id, &mut data)?;
        buf[..count].copy_from_slice(&data[start..start + count]);
        self.advance(count);
        Ok(count)
    }

    /// Write the partial head or tail left around an aligned batched request.
    fn write_partial(&mut self, buf: &[u8]) -> DevResult<usize> {
        debug_assert!(self.offset != 0 || buf.len() < BLOCK_SIZE);
        let mut data = [0u8; BLOCK_SIZE];
        let start = self.offset;
        let count = buf.len().min(BLOCK_SIZE - self.offset);

        self.dev
            .read_block(self.base_block + self.block_id, &mut data)?;
        data[start..start + count].copy_from_slice(&buf[..count]);
        self.dev
            .write_block(self.base_block + self.block_id, &data)?;
        self.advance(count);
        Ok(count)
    }

    fn advance(&mut self, count: usize) {
        self.offset += count;
        if self.offset == BLOCK_SIZE {
            self.block_id += 1;
            self.offset = 0;
        }
    }
}

impl KernelDevOp for Disk {
    type DevType = Disk;

    /// 从块设备读取数据
    fn read(dev: &mut Self::DevType, mut buf: &mut [u8]) -> Result<usize, i32> {
        let mut total_len = 0;
        while !buf.is_empty() {
            // BlockDevice accepts a multi-block buffer. Preserve partial-block
            // handling, but submit every aligned contiguous span as one
            // VirtIO request instead of one request per 512-byte sector.
            if dev.offset == 0 {
                let whole_blocks_len = buf.len() / BLOCK_SIZE * BLOCK_SIZE;
                if whole_blocks_len != 0 {
                    dev.dev
                        .read_block(
                            dev.base_block + dev.block_id,
                            &mut buf[..whole_blocks_len],
                        )
                        .map_err(|_| -1)?;
                    dev.block_id += whole_blocks_len / BLOCK_SIZE;
                    total_len += whole_blocks_len;
                    let remaining = buf;
                    buf = &mut remaining[whole_blocks_len..];
                    continue;
                }
            }
            if let Ok(len) = dev.read_partial(buf) {
                if len == 0 {
                    break;
                }
                let tmp = buf;
                buf = &mut tmp[len..]; // 推进指针（借用）
                total_len += len;
            } else {
                return Err(-1);
            }
        }
        Ok(total_len)
    }

    /// 向块设备写入数据
    fn write(dev: &mut Self::DevType, mut buf: &[u8]) -> Result<usize, i32> {
        let mut total_len = 0;
        while !buf.is_empty() {
            if dev.offset == 0 {
                let whole_blocks_len = buf.len() / BLOCK_SIZE * BLOCK_SIZE;
                if whole_blocks_len != 0 {
                    dev.dev
                        .write_block(
                            dev.base_block + dev.block_id,
                            &buf[..whole_blocks_len],
                        )
                        .map_err(|_| -1)?;
                    dev.block_id += whole_blocks_len / BLOCK_SIZE;
                    total_len += whole_blocks_len;
                    buf = &buf[whole_blocks_len..];
                    continue;
                }
            }
            if let Ok(len) = dev.write_partial(buf) {
                if len == 0 {
                    break;
                }
                buf = &buf[len..]; // 推进指针（借用）
                total_len += len;
            } else {
                return Err(-1);
            }
        }
        Ok(total_len)
    }

    fn flush(dev: &mut Self::DevType) -> Result<usize, i32> {
        dev.dev.flush().map_err(|_| -1)?;
        Ok(0)
    }

    fn seek(dev: &mut Self::DevType, off: i64, whence: i32) -> Result<i64, i32> {
        let size = dev.size();
        let new_pos = match whence as u32 {
            lwext4_rust::bindings::SEEK_SET => Some(off),
            lwext4_rust::bindings::SEEK_CUR => dev
                .position()
                .checked_add_signed(off as isize)
                .map(|v| v as i64),
            lwext4_rust::bindings::SEEK_END => {
                size.checked_add_signed(off as isize).map(|v| v as i64)
            }
            _ => return Err(-1),
        }
        .ok_or(-1)?;

        if new_pos < 0 {
            return Err(-1);
        }

        if new_pos as usize > size {
            println!("[kernel] WARNING: Seek beyond the end of the block device!!!");
        }

        dev.set_position(new_pos as usize);
        Ok(new_pos)
    }
}
