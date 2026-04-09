// block_cache.rs

//! 操作块的基本单元, 管理元数据——读写块组描述符、超级块
use lazy_static::*;
use spin::RwLock;
use alloc::sync::Arc;
use alloc::collections::VecDeque;
use crate::config::{ BLOCK_SIZE, BLOCK_CACHE_LIMIT };
use super::BlockDevice;

lazy_static! {
    /// 全局块缓存管理器
    pub static ref BLOCK_CACHE_MANAGER: RwLock<BlockCacheManager> =
        RwLock::new(BlockCacheManager::new());
}

/// 块缓存
/// 
/// 设备块在内存中的抽象
pub struct BlockCache {
    cache: [u8; BLOCK_SIZE],
    block_id: usize,
    block_device: Arc<dyn BlockDevice>,
    modified: bool,
}

impl BlockCache {
    /// 从磁盘上加载一个块缓存
    pub fn new(block_id: usize, block_device: Arc<dyn BlockDevice>) -> Self {
        let mut cache = [0u8; BLOCK_SIZE];
        block_device.read_block(block_id, &mut cache);
        Self {
            cache,
            block_id,
            block_device,
            modified: false,
        }
    }

    /// 得到块缓存中指定偏移量 offset 的地址
    fn addr_of_offset(&self, offset: usize) -> usize {
        &self.cache[offset] as *const _ as usize
    }

    // 得到块缓存中指定偏移量 offset 上的某个类型的数据的不可变借用
    pub fn get_ref<T>(&self, offset: usize) -> &T
    where
        T: Sized,
    {
        let type_size = core::mem::size_of::<T>();
        assert!(offset + type_size <= BLOCK_SIZE);
        let addr = self.addr_of_offset(offset);
        unsafe { &*(addr as *const T) }
    }

    // 得到块缓存中指定偏移量 offset 上的某个类型的数据的可变借用
    pub fn get_mut<T>(&mut self, offset: usize) -> &mut T
    where
        T: Sized,
    {
        let type_size = core::mem::size_of::<T>();
        assert!(offset + type_size <= BLOCK_SIZE);
        self.modified = true;
        let addr = self.addr_of_offset(offset);
        unsafe { &mut *(addr as *mut T) }
    }

    /// 使用块缓存中指定偏移量 offset 上的某个类型的数据
    pub fn read<T, V>(&self, offset: usize, f: impl FnOnce(&T) -> V) -> V {
        f(self.get_ref(offset))
    }

    /// 修改块缓存中指定偏移量 offset 上的某个类型的数据
    pub fn modify<T, V>(&mut self, offset: usize, f: impl FnOnce(&mut T) -> V) -> V {
        f(self.get_mut(offset))
    }

    /// 同步缓存块和磁盘
    pub fn sync(&mut self) {
        if self.modified {
            self.modified = false;
            self.block_device.write_block(self.block_id, &self.cache);
        }
    }
}

// 释放即写回
impl Drop for BlockCache {
    fn drop(&mut self) {
        self.sync()
    }
}

/// 块缓存管理器
pub struct BlockCacheManager {
    queue: VecDeque<(usize, Arc<RwLock<BlockCache>>)>,
}

impl BlockCacheManager {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
    pub fn get_block_cache(
        &mut self,
        block_id: usize,
        block_device: Arc<dyn BlockDevice>,
    ) -> Arc<RwLock<BlockCache>> {
        if let Some(pair) = self.queue.iter()
        .find(|pair| block_id == pair.0) {
            // 找对应的块缓存
            Arc::clone(&pair.1)
        } else {
            if self.queue.len() == BLOCK_CACHE_LIMIT {
                if let Some((idx, _)) = self
                    .queue
                    .iter()
                    .enumerate()
                    .find(|(_, pair)| Arc::strong_count(&pair.1) == 1)
                {
                    self.queue.drain(idx..=idx);
                } else {
                    panic!("Run out of BLOCK_CACHE!");
                }
            }
            let block_cache = Arc::new(RwLock::new(BlockCache::new(
                block_id,
                Arc::clone(&block_device),
            )));
            self.queue
                .push_back((block_id, Arc::clone(&block_cache)));
            block_cache
        }
    }
}

/// 获取设备快对应的块缓存
pub fn get_block_cache(
    fs_block_id: usize,
    block_device: Arc<dyn BlockDevice>,
) -> Arc<RwLock<BlockCache>> {
    BLOCK_CACHE_MANAGER
        .write()
        .get_block_cache(fs_block_id, block_device)
}

/// 同步所有缓存块
#[allow(unused)]
pub fn block_cache_sync_all() {
    let manager = BLOCK_CACHE_MANAGER.write();
    for (_, cache) in manager.queue.iter() {
        cache.write().sync();
    }
}
