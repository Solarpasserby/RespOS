// os/src/fs/pipe.rs

use super::KStat;
use super::vfs::{FileOp, InodeType, OpenFlags};
use crate::config::PIPE_BUFFER_SIZE;
use crate::syscall::{Errno, SysResult};
use crate::task::yield_current_task;
use alloc::sync::Arc;
use core::any::Any;
use spin::Mutex;

const PIPE_INO: u64 = 0x1000;
const PIPE_DEV: u64 = 0x200;

pub struct Pipe {
    buffer: Arc<Mutex<PipeRingBuffer>>,
    readable: bool,
    writable: bool,
}

impl Pipe {
    /// return (pipe_read, pipe_write)
    fn read_end_with_buffer(buffer: Arc<Mutex<PipeRingBuffer>>) -> Self {
        Self {
            readable: true,
            writable: false,
            buffer,
        }
    }
    fn write_end_with_buffer(buffer: Arc<Mutex<PipeRingBuffer>>) -> Self {
        Self {
            readable: false,
            writable: true,
            buffer,
        }
    }

    pub fn read_inner(&self, buf: &mut [u8]) -> usize {
        let mut read_size = 0;
        let mut buffer = self.buffer.lock();
        for char in buf {
            if buffer.status != RingBufferStatus::EMPTY {
                *char = buffer.read_byte();
                read_size += 1;
            } else {
                break;
            }
        }
        read_size
    }
    pub fn write_inner(&self, buf: &[u8]) -> usize {
        let mut write_size = 0;
        let mut buffer = self.buffer.lock();
        for char in buf {
            if buffer.status != RingBufferStatus::FULL {
                buffer.write_byte(*char);
                write_size += 1;
            } else {
                break;
            }
        }
        write_size
    }
}

impl FileOp for Pipe {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        loop {
            let ret = self.read_inner(buf);
            if ret != 0 {
                return Ok(ret);
            } else if self.buffer.lock().write_closed() {
                // 缓存为空且写端关闭
                return Ok(0);
            } else {
                // 缓存为空但存在写端
                yield_current_task();
                continue;
            }
        }
    }
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        loop {
            if self.buffer.lock().read_closed() {
                // 读端关闭
                return Err(Errno::EPIPE);
            } else {
                let ret = self.write_inner(buf);
                if ret != 0 {
                    return Ok(ret);
                } else {
                    // 缓存已满但读端存在
                    yield_current_task();
                    continue;
                }
            }
        }
    }
    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }
    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }
    fn get_offset(&self) -> usize {
        0
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn get_flags(&self) -> OpenFlags {
        OpenFlags::empty()
    }
    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Fifo)
            .with_dev(PIPE_DEV)
            .with_ino(PIPE_INO)
            .with_mode(0o666))
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // 当管道可写，销毁时其对应的管道缓存写端标记为不存在
        if self.readable {
            let mut buffer = self.buffer.lock();
            buffer.read_closed = true;
        }
        // 当管道可写，销毁时其对应的管道缓存写端标记为不存在
        if self.writable {
            let mut buffer = self.buffer.lock();
            buffer.write_closed = true;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum RingBufferStatus {
    FULL,
    EMPTY,
    NORMAL,
}

struct PipeRingBuffer {
    buffer: [u8; PIPE_BUFFER_SIZE],
    head: usize,
    tail: usize,
    status: RingBufferStatus,
    read_closed: bool,  // 管道读端是否关闭
    write_closed: bool, // 管道写端是否关闭
}

impl PipeRingBuffer {
    pub fn new() -> Self {
        Self {
            buffer: [0; PIPE_BUFFER_SIZE],
            head: 0,
            tail: 0,
            status: RingBufferStatus::EMPTY,
            read_closed: false,
            write_closed: false,
        }
    }

    fn read_byte(&mut self) -> u8 {
        assert_ne!(self.status, RingBufferStatus::EMPTY);
        self.status = RingBufferStatus::NORMAL;
        let byte = self.buffer[self.head];
        self.head = (self.head + 1) % PIPE_BUFFER_SIZE;
        if self.head == self.tail {
            self.status = RingBufferStatus::EMPTY;
        }
        byte
    }
    fn write_byte(&mut self, byte: u8) {
        assert_ne!(self.status, RingBufferStatus::FULL);
        self.status = RingBufferStatus::NORMAL;
        self.buffer[self.tail] = byte;
        self.tail = (self.tail + 1) % PIPE_BUFFER_SIZE;
        if (self.tail + 1) % PIPE_BUFFER_SIZE == self.head {
            self.status = RingBufferStatus::FULL;
        }
    }
    fn read_closed(&self) -> bool {
        self.read_closed
    }
    fn write_closed(&self) -> bool {
        self.write_closed
    }
}

pub fn make_pipe() -> (Arc<Pipe>, Arc<Pipe>) {
    let buffer = Arc::new(Mutex::new(PipeRingBuffer::new()));
    let read_end = Arc::new(Pipe::read_end_with_buffer(buffer.clone()));
    let write_end = Arc::new(Pipe::write_end_with_buffer(buffer.clone()));
    (read_end, write_end)
}
