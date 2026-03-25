// os/src/loader.rs
// FIXME: 删了很多之前写的，心痛~

/// 获取用户程序个数
pub fn get_app_num() -> usize {
    unsafe extern "C" {
        safe fn _num_app();
    }
    unsafe { (_num_app as *const usize).read_volatile() }
}

/// 获取用户程序字节数据
/// 
/// 依赖于汇编提供的符号地址
pub fn get_app_data(app_id: usize) -> &'static [u8] {
    unsafe extern "C" { safe fn _num_app(); }
    let app_ptr = _num_app as *const usize;
    let app_num = unsafe { app_ptr.read_volatile() };
    let app_start = unsafe {
        core::slice::from_raw_parts(app_ptr.add(1), app_num + 1)
    };
    assert!(app_id < app_num, "[kernel] Failed to get app data due to bad app id!");
    unsafe {
        core::slice::from_raw_parts(
            app_start[app_id] as *const u8,
            app_start[app_id + 1] - app_start[app_id]
        )
    }
}
