// user/src/lang_item.rs

use core::{arch::asm, ptr};

#[panic_handler]
fn panic_handler(panic_info: &core::panic::PanicInfo) -> ! {
    let err = panic_info.message();
    if let Some(location) = panic_info.location() {
        println!(
            "Panicked at {}:{}, {}",
            location.file(),
            location.line(),
            err
        );
        unsafe {
            trace_and_print_user_stack();
        }
    } else {
        println!("Panicked: {}", err);
    }
    loop {}
}

// 尝试实现崩溃时，对栈上内容的回溯，对定位错误有一定帮助
// 即显示调用栈，虽然只有地址
unsafe fn trace_and_print_user_stack() {
    let mut fp: *const usize;
    unsafe {
        #[cfg(target_arch = "riscv64")]
        asm!("mv {}, fp", out(reg) fp);
        #[cfg(target_arch = "loongarch64")]
        asm!("or {}, $r22, $zero", out(reg) fp);
    }

    println!("----- start tracing ustack -----");
    while fp != ptr::null() {
        unsafe {
            let fra = *fp.sub(1);
            let ffp = *fp.sub(2);

            println!("0x{:016x}, fp = 0x{:016x}", fra, ffp);

            fp = ffp as *const usize;
        }
    }
    println!("----- finish traceing ustack -----");
}
