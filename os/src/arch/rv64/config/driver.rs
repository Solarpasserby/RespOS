// os/src/config/driver.rs

pub const BLOCK_SIZE: usize = 512;
// TODO: 不会找一个平衡的值
// 设置块缓存管理队列长度
pub const BLOCK_CACHE_LIMIT: usize = 128;
