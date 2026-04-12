// os/src/drivers/disk.rs

use alloc::sync::Arc;
use crate::config::BLOCK_SIZE;
use super::{BlockDevice, DevResult};

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
        self.offset = pos as usize % BLOCK_SIZE;
    }

    /// 读取单个块数据，返回读取的字节数
    pub fn read_one(&mut self, buf: &mut [u8]) -> DevResult<usize> {
        // TODO： 没有引入 log 模块，计划之后添加以优化内核程序输出
        // info!("block id: {}", self.block_id);
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
