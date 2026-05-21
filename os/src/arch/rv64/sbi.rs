// src/sbi.rs

//! ### SBI 模块
//!
//! 调用 SBI 的服务，实现一些更底层的操作，并封装成函数使用

/// 设置 mtimecmp ，使指定时钟周期产生时钟中断
pub fn set_timer(time_value: usize) {
    sbi_rt::set_timer(time_value as _);
}

/// 向终端打印字符
pub fn console_putchar(c: usize) {
    #[allow(deprecated)] // TODO: 被弃用的接口，但是胜在简单，之后可以试着重写
    sbi_rt::legacy::console_putchar(c);
    // let temp = sbi_rt::console_write(bytes) // TODO: 新接口不知道怎么用
    // if temp.error != 0 { panic!("omg") }
}

/// 向终端打印字符
pub fn console_getchar() -> usize {
    #[allow(deprecated)]
    sbi_rt::legacy::console_getchar()
}

/// 关闭机器
pub fn shutdown(failure: bool) -> ! {
    use sbi_rt::{NoReason, Shutdown, SystemFailure, system_reset};
    if !failure {
        system_reset(Shutdown, NoReason);
    } else {
        system_reset(Shutdown, SystemFailure);
    }
    unreachable!()
}
