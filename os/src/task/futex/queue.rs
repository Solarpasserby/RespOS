// os/src/task/futex/queue.rs

use crate::mutex::SpinNoIrqLock;
use alloc::collections::vec_deque::VecDeque;
use alloc::vec::Vec;
use lazy_static::lazy_static;

const FUTEX_HASH_SIZE: usize = 256;

lazy_static! {
    pub static ref FUTEX_QUEUES: SpinNoIrqLock<FutexQueues> =
        SpinNoIrqLock::new(FutexQueues::new());
}

/// futex 键，唯一标识一个 futex 等待地址。
#[derive(Clone, PartialEq, Eq)]
pub struct FutexKey {
    /// 私有 futex 使用线程组号；共享 futex 暂统一为 0。
    pub scope: usize,
    /// 用户空间 futex 地址。
    pub uaddr: usize,
}

/// 等待队列条目。
pub struct FutexQ {
    pub key: FutexKey,
    /// 等待线程的 tid。
    pub tid: usize,
    pub bitset: u32,
}

/// 全局 futex 等待队列，256 个哈希桶，每个桶一个 VecDeque。
pub struct FutexQueues {
    buckets: Vec<VecDeque<FutexQ>>,
}

impl FutexQueues {
    pub fn new() -> Self {
        let mut buckets = Vec::with_capacity(FUTEX_HASH_SIZE);
        for _ in 0..FUTEX_HASH_SIZE {
            buckets.push(VecDeque::new());
        }
        Self { buckets }
    }

    pub fn bucket_by_idx(&mut self, idx: usize) -> &mut VecDeque<FutexQ> {
        &mut self.buckets[idx]
    }
}

fn futex_hash(uaddr: usize) -> usize {
    let h = uaddr.wrapping_mul(0x9e370001) >> 12;
    h & (FUTEX_HASH_SIZE - 1)
}

pub fn futex_hash_idx(uaddr: usize) -> usize {
    futex_hash(uaddr)
}
