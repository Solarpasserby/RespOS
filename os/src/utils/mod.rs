mod boot;

use alloc::string::String;

pub use boot::enter_main;

/// 将 C 风格的字符串转换为 Rust 型字符串
/// Convert C-style string(end with '\0') to rust string
pub fn c_str_to_string(ptr: *const u8) -> String {
    assert!(
        !ptr.is_null(),
        "[kernel] c_str_to_string: null pointer passed in, please check!"
    );
    let mut ptr = ptr as usize;
    let mut ret = String::new();
    // trace!("[c_str_to_string] convert ptr at {:#x} to string", ptr);
    loop {
        // 由调用者保证传入 ptr 的合法性
        // TODO: 之后将采用更安全的方式读取用户数据
        let ch: u8 = unsafe { *(ptr as *const u8) };
        if ch == 0 {
            break;
        }
        ret.push(ch as char);
        ptr += 1;
    }
    ret
}
