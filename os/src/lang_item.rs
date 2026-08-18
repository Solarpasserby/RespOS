// os/src/lang_item.rs

// 主要处理 Rust 内部语言逻辑

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::platform::report_panic(info);
    crate::platform::shutdown(true)
}
