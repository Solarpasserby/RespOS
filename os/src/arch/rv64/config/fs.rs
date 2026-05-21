// os/src/config/fs.rs

/// 最大文件描述符值——先这么设计
pub const FTB_RLIMIT: usize = 1024;

/// 管道缓存大小
pub const PIPE_BUFFER_SIZE: usize = 4096;
