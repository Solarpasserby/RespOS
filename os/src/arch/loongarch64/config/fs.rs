// 文件系统配置

/// 最大文件描述符数量
pub const FTB_RLIMIT: usize = 1024;

/// 管道缓冲区大小
pub const PIPE_BUFFER_SIZE: usize = 64 * 1024;

/// inode 缓存容量上限
pub const INODE_CACHE_CAPACITY: usize = 1024;

/// 目录项缓存容量上限
pub const DENTRY_CACHE_CAPACITY: usize = 16 * 1024;

/// 页缓存容量上限
/// Frame-backed file cache budget. Cache data consumes physical frames rather
/// than the fixed kernel heap; 32K pages retain a 128 MiB working set.
pub const PAGE_CACHE_GLOBAL_MAX_PAGES: usize = 32 * 1024;
