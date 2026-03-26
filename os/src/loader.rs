// os/src/loader.rs
// FIXME: 删了很多之前写的，心痛~

use lazy_static::lazy_static;
use alloc::vec::Vec;

lazy_static! {
    /// 初始化用户程序名数组
    static ref APP_NAMES: Vec<&'static str> = {
        let app_num = get_app_num();
        unsafe extern "C" { safe fn _app_names(); }
        let mut start = _app_names as *const() as *const u8;
        let mut v = Vec::new();
        unsafe {
            for _ in 0..app_num {
                let mut end = start;
                while end.read_volatile() != ('\0' as u8) { end = end.add(1); }
                let slice = core::slice::from_raw_parts(start, end as usize - start as usize);
                let str = core::str::from_utf8(slice).unwrap();
                v.push(str);
                start = end.add(1);
            }
        }
        v
    };
}

/// 获取用户程序个数
pub fn get_app_num() -> usize {
    unsafe extern "C" {
        safe fn _app_num();
    }
    unsafe { (_app_num as *const usize).read_volatile() }
}

/// 获取用户程序字节数据
/// 
/// 依赖于汇编提供的符号地址
pub fn get_app_data(app_id: usize) -> &'static [u8] {
    unsafe extern "C" { safe fn _app_num(); }
    let app_ptr = _app_num as *const usize;
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

/// 根据用户程序名获得用户程序数据
pub fn get_app_data_by_name(name: &str) -> Option<&'static [u8]> {
    let app_num = get_app_num();
    (0..app_num)
        .find(|&i| APP_NAMES[i] == name)
        .map(|i| get_app_data(i))
}

/// 罗列所有的用户程序
pub fn list_apps() {
    println!("/**** APPS ****");
    for app in APP_NAMES.iter() {
        println!("{}", app);
    }
    println!("**************/");
    ()
}
