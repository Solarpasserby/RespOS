// os/src/lang_item.rs

// 主要处理 Rust 内部语言逻辑

use super::sbi::shutdown;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    #[cfg(feature = "board_ls2k1000")]
    {
        // 真机：println! 走 CONSOLE_OUTPUT_LOCK + 预编译 core 的 fmt（+ual 非对齐代码），
        // 在 trap 上下文中会死锁/二次异常，导致 panic 消息永远打不出来。这里改为无锁
        // 逐字节直写 uncached UART，手工格式化；只打印一次（若打印路径本身再异常，
        // 直接 shutdown，避免无限递归）。
        use core::sync::atomic::{AtomicBool, Ordering};
        static PANIC_PRINTED: AtomicBool = AtomicBool::new(false);
        if !PANIC_PRINTED.swap(true, Ordering::Relaxed) {
            super::sbi::early_print("Panicked at ");
            if let Some(location) = info.location() {
                super::sbi::early_print(location.file());
                super::sbi::early_print(":");
                super::sbi::early_print_hex(location.line() as usize);
            }
            super::sbi::early_print("\n");
        }
    }
    #[cfg(not(feature = "board_ls2k1000"))]
    {
        if let Some(location) = info.location() {
            println!(
                "Panicked at {}:{} {}",
                location.file(),
                location.line(),
                info.message()
            );
        } else {
            println!("Panicked: {}", info.message());
        }
    }

    shutdown(true)
}
