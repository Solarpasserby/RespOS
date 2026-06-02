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
}

impl Disk {
    pub fn new(dev: Arc<dyn BlockDevice>) -> Self {
        assert_eq!(BLOCK_SIZE, dev.block_size());
        Self {
            block_id: 0,
            offset: 0,
            dev,
        }
    }

    /// 获取总数据大小
    pub fn size(&self) -> usize {
        self.dev.num_blocks() * BLOCK_SIZE
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

    /// 读取单个块数据，返回读取的字节数
    pub fn read_one(&mut self, buf: &mut [u8]) -> DevResult<usize> {
        let read_size = if self.offset == 0 && buf.len() >= BLOCK_SIZE {
            // 读取完整的块
            self.dev
                .read_block(self.block_id, &mut buf[0..BLOCK_SIZE])?;
            self.block_id += 1;
            BLOCK_SIZE
        } else {
            // 读取局部的块
            let mut data = [0u8; BLOCK_SIZE];
            let start = self.offset;
            let count = buf.len().min(BLOCK_SIZE - self.offset);
            // if start > BLOCK_SIZE { info!("block size: {} start {}", BLOCK_SIZE, start); }

            self.dev.read_block(self.block_id, &mut data)?;
            buf[..count].copy_from_slice(&data[start..start + count]);

            self.offset += count;
            if self.offset >= BLOCK_SIZE {
                self.block_id += 1;
                self.offset -= BLOCK_SIZE;
            }
            count
        };
        Ok(read_size)
    }

    /// 写入单个块数据，返回写入的字节数
    pub fn write_one(&mut self, buf: &[u8]) -> DevResult<usize> {
        let write_size = if self.offset == 0 && buf.len() >= BLOCK_SIZE {
            // whole block
            self.dev.write_block(self.block_id, &buf[0..BLOCK_SIZE])?;
            self.block_id += 1;
            BLOCK_SIZE
        } else {
            // partial block
            let mut data = [0u8; BLOCK_SIZE];
            let start = self.offset;
            let count = buf.len().min(BLOCK_SIZE - self.offset);

            self.dev.read_block(self.block_id, &mut data)?;
            data[start..start + count].copy_from_slice(&buf[..count]);
            self.dev.write_block(self.block_id, &data)?;

            self.offset += count;
            if self.offset >= BLOCK_SIZE {
                self.block_id += 1;
                self.offset -= BLOCK_SIZE;
            }
            count
        };
        Ok(write_size)
    }

    /// 依据总偏移读取对应块数据
    pub fn read_offset(&mut self, offset: usize) -> [u8; BLOCK_SIZE] {
        let block_id = offset / BLOCK_SIZE;
        let mut block_data = [0u8; BLOCK_SIZE];
        self.dev.read_block(block_id, &mut block_data).unwrap();
        block_data
    }

    /// 依据总偏移读取对应块数据，只能写入完整的块
    pub fn write_offset(&mut self, offset: usize, buf: &[u8]) -> DevResult<usize> {
        assert!(
            buf.len() == BLOCK_SIZE,
            "Buffer length must be equal to BLOCK_SIZE"
        );
        assert!(offset % BLOCK_SIZE == 0);
        let block_id = offset / BLOCK_SIZE;
        self.dev.write_block(block_id, buf).unwrap();
        Ok(buf.len())
    }
}

impl KernelDevOp for Disk {
    type DevType = Disk;

    /// 从块设备读取数据
    fn read(dev: &mut Self::DevType, mut buf: &mut [u8]) -> Result<usize, i32> {
        let mut total_len = 0;
        while !buf.is_empty() {
            if let Ok(len) = dev.read_one(buf) {
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
            if let Ok(len) = dev.write_one(buf) {
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
