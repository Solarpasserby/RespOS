// os/src/fs/pipe.rs

use super::KStat;
use super::vfs::InodeType;
use super::{FileOp, OpenFlags};
use crate::config::PIPE_BUFFER_SIZE;
use crate::syscall::{Errno, SysResult};
use crate::task::{
    current_task, prepare_current_task_blocked, remove_task, switch_to_next_task, wakeup_task,
    yield_current_task,
};
use alloc::{collections::VecDeque, sync::Arc};
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
        if buf.is_empty() {
            return Ok(0);
        }
        let task = current_task().expect("[kernel] current task is None.");
        loop {
            let mut wake_writer = None;
            let mut should_block = false;
            // —— 在锁内尝试读取 + 决定是否阻塞 ——
            // 读成功 → 唤醒一个写端等待者（缓冲区有空间了）
            // 读失败且写端已关 → 返回 0（EOF）
            // 读失败且写端还在 → 将自己加入读等待队列，切走
            let ret = {
                let mut buffer = self.buffer.lock();
                let mut read_size = 0;
                for ch in buf.iter_mut() {
                    if buffer.status == RingBufferStatus::EMPTY {
                        break;
                    }
                    *ch = buffer.read_byte();
                    read_size += 1;
                }

                if read_size != 0 {
                    // 读到数据：缓冲区腾出空间，唤醒一个写端等待者
                    wake_writer = buffer.pop_write_waiter();
                    read_size
                } else if buffer.write_closed() {
                    // 缓冲区空且写端已关：EOF，直接返回
                    return Ok(0);
                } else {
                    // 缓冲区空但写端还在：需要阻塞等待数据
                    task.set_interruptible(true);
                    buffer.push_read_waiter(task.tid());
                    should_block = prepare_current_task_blocked();
                    if !should_block {
                        // 入队后但在切走前，信号已到达 → 撤销阻塞
                        task.set_interruptible(false);
                        buffer.remove_read_waiter(task.tid());
                    }
                    0
                }
            };

            if let Some(tid) = wake_writer {
                wakeup_task(tid);
            }
            if ret != 0 {
                return Ok(ret);
            }
            if should_block {
                // 在我们切走之前，写端已经写入数据并唤醒我们，此时从调度队列中移除即可，无需实际切换。
                if task.is_ready() {
                    remove_task(task.tid());
                    task.set_running();
                } else {
                    switch_to_next_task();
                }
                // 回来后清理等待队列残留并检查信号中断
                self.buffer.lock().remove_read_waiter(task.tid());
                task.set_interruptible(false);
                if task.is_interrupted() || task.check_signal_interrupt() {
                    task.clear_interrupted();
                    return Err(Errno::EINTR);
                }
            } else {
                // prepare_current_task_blocked 返回 false：
                // 缓冲区可能在我们入队前恰好被写端填了数据，yield 让出 CPU 后重试
                yield_current_task();
            }
        }
    }
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let task = current_task().expect("[kernel] current task is None.");
        loop {
            let mut wake_reader = None;
            let mut should_block = false;
            // —— 在锁内尝试写入 + 决定是否阻塞 ——
            // 写成功 → 唤醒一个读端等待者（有数据可读了）
            // 写失败且缓冲区满 → 将自己加入写等待队列，切走
            // 读端已关闭 → EPIPE（对端不存在，写无意义）
            let ret = {
                let mut buffer = self.buffer.lock();
                if buffer.read_closed() {
                    return Err(Errno::EPIPE);
                }

                let mut write_size = 0;
                for ch in buf {
                    if buffer.status == RingBufferStatus::FULL {
                        break;
                    }
                    buffer.write_byte(*ch);
                    write_size += 1;
                }

                if write_size != 0 {
                    // 写入了数据：唤醒一个读端等待者来消费
                    wake_reader = buffer.pop_read_waiter();
                    write_size
                } else {
                    // 缓冲区满但读端还在：需要阻塞等待空间
                    task.set_interruptible(true);
                    buffer.push_write_waiter(task.tid());
                    should_block = prepare_current_task_blocked();
                    if !should_block {
                        task.set_interruptible(false);
                        buffer.remove_write_waiter(task.tid());
                    }
                    0
                }
            };

            if let Some(tid) = wake_reader {
                wakeup_task(tid);
            }
            if ret != 0 {
                return Ok(ret);
            }
            if should_block {
                if task.is_ready() {
                    remove_task(task.tid());
                    task.set_running();
                } else {
                    switch_to_next_task();
                }
                self.buffer.lock().remove_write_waiter(task.tid());
                task.set_interruptible(false);
                if task.is_interrupted() || task.check_signal_interrupt() {
                    task.clear_interrupted();
                    return Err(Errno::EINTR);
                }
            } else {
                yield_current_task();
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

    // 管道读端是否有数据可立即读取而不阻塞
    // 缓冲区非空，或者写端已关闭
    fn read_ready(&self) -> bool {
        let buf = self.buffer.lock();
        buf.status != RingBufferStatus::EMPTY || buf.write_closed
    }
    // 管道写端是否有空间可立即写入而不阻塞
    fn write_ready(&self) -> bool {
        self.buffer.lock().status != RingBufferStatus::FULL
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
        // 管道关闭时：
        // - 标记己端已关闭，让对端后续 read/write 感知到
        // - 收集被阻塞在对端缓冲区上的所有等待者并全部唤醒
        //   例：读端关闭 → 写端阻塞在 write_waiters 中 → 必须唤醒，否则永久挂起
        let mut wake_waiters = VecDeque::new();
        if self.readable {
            let mut buffer = self.buffer.lock();
            buffer.read_closed = true;
            wake_waiters.append(&mut buffer.write_waiters);
        }
        if self.writable {
            let mut buffer = self.buffer.lock();
            buffer.write_closed = true;
            wake_waiters.append(&mut buffer.read_waiters);
        }
        for tid in wake_waiters {
            wakeup_task(tid);
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
    read_waiters: VecDeque<usize>,
    write_waiters: VecDeque<usize>,
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
            read_waiters: VecDeque::new(),
            write_waiters: VecDeque::new(),
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
    /// 将读端 tid 加入等待队列（去重，避免同任务重复入队）
    fn push_read_waiter(&mut self, tid: usize) {
        if !self.read_waiters.iter().any(|&waiter| waiter == tid) {
            self.read_waiters.push_back(tid);
        }
    }
    /// 将写端 tid 加入等待队列（去重）
    fn push_write_waiter(&mut self, tid: usize) {
        if !self.write_waiters.iter().any(|&waiter| waiter == tid) {
            self.write_waiters.push_back(tid);
        }
    }
    /// FIFO 弹出最早阻塞的读端
    fn pop_read_waiter(&mut self) -> Option<usize> {
        self.read_waiters.pop_front()
    }
    /// FIFO 弹出最早阻塞的写端
    fn pop_write_waiter(&mut self) -> Option<usize> {
        self.write_waiters.pop_front()
    }
    /// 信号打断 / 竞态回退时从队列中移除特定等待者
    fn remove_read_waiter(&mut self, tid: usize) {
        self.read_waiters.retain(|&waiter| waiter != tid);
    }
    fn remove_write_waiter(&mut self, tid: usize) {
        self.write_waiters.retain(|&waiter| waiter != tid);
    }
}

pub fn make_pipe() -> (Arc<Pipe>, Arc<Pipe>) {
    let buffer = Arc::new(Mutex::new(PipeRingBuffer::new()));
    let read_end = Arc::new(Pipe::read_end_with_buffer(buffer.clone()));
    let write_end = Arc::new(Pipe::write_end_with_buffer(buffer.clone()));
    (read_end, write_end)
}
