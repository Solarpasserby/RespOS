// 文件系统配置

/// 最大文件描述符数量
pub const FTB_RLIMIT: usize = 1024;

/// 管道缓冲区大小
pub const PIPE_BUFFER_SIZE: usize = 4096;

/// inode 缓存容量上限
pub const INODE_CACHE_CAPACITY: usize = 1024;

/// 目录项缓存容量上限
pub const DENTRY_CACHE_CAPACITY: usize = 1024;

/// 页缓存容量上限
pub const PAGE_CACHE_GLOBAL_MAX_PAGES: usize = 512;
