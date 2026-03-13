// os/src/config.rs

//! ### 内核主要配置模块

// 栈大小
pub const USER_STACK_SIZE: usize = 4096 * 2;
pub const KERNEL_STACK_SIZE: usize = 4096 * 2;

// 用户程序地址设置
pub const APP_BASE_ADDRESS: usize = 0x80400000;
pub const APP_SIZE_LIMIT: usize = 0x20000;

// TODO: 简陋的用户程序数量设置，主要控制栈数量
pub const MAX_APP_NUM: usize = 16;

// 时钟频率，与硬件设备相关
pub const CLOCK_FREQ: usize = 12500000;