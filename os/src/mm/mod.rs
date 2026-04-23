// os/src/mm.rs

//! ### 内存管理模块
//! 
//! 实现虚拟地址空间
//! 
//! 这部分内容繁多，建立了多层的抽象，隐含了很多深远的设计思想，需要好好消化

mod heap_allocator;
mod frame_allocator;
mod address;
mod page_table;
mod memory_set;

use alloc::string::String;
use heap_allocator::init_heap;
use frame_allocator::init_frame_allocator;
use crate::task::current_task;
use crate::syscall::{SysResult, Errno};
pub use address::*;
pub use frame_allocator::{FrameTracker, frame_alloc};
pub use page_table::{PageTableEntry, PageTable};
pub use memory_set::{KERNEL_SPACE, MemorySet, MapPermission};

const USER_CSTR_MAX_LEN: usize = 4096;


/// 初始化内存管理，启用虚拟地址
pub fn init() {
    init_heap();
    init_frame_allocator();
    KERNEL_SPACE.lock().activate();
    // 注意此时已经启用了虚拟地址
}

pub fn copy_from_user<T: Copy>(dst: *mut T, src: *const T, len: usize) -> SysResult<usize> {
    if dst.is_null() || src.is_null() {
        return Err(Errno::EFAULT);
    }
    if len == 0 { return Ok(0); }

    // 检查地址是否合法
    let byte_len = len // 防止溢出
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    let end = (src as usize) // 防止溢出
        .checked_add(byte_len)
        .ok_or(Errno::EFAULT)?;
    let start_vpn = VirtAddr::from(src as usize).floor();
    let end_vpn = VirtAddr::from(end).ceil();
    let vpn_range = VPNRange::new(start_vpn, end_vpn);
    current_task()
        .expect("[kernel] current task is None.")
        .inner_exclusive_access()
        .memory_set
        .check_valid_user_vpn_range(vpn_range, MapPermission::READ)?;
    // 执行复制
    unsafe {
        let src_slice = core::slice::from_raw_parts(src, len);
        let dst_slice = core::slice::from_raw_parts_mut(dst, len);
        dst_slice.copy_from_slice(src_slice);
    }
    Ok(len)
}

/// 将 C 风格的字符串转换为 Rust 型字符串
pub fn copy_cstr_from_user(ptr: *const u8) -> SysResult<String> {
    if ptr.is_null() {
        return Err(Errno::EFAULT);
    }

    let start_vpn = VirtAddr::from(ptr as usize).floor();

    let vpn_range = current_task()
        .expect("[kernel] current task is None.")
        .inner_exclusive_access()
        .memory_set
        .check_valid_user_vpn(start_vpn, MapPermission::READ)?;

    let area_end = usize::from(VirtAddr::from(vpn_range.get_end()));
    let max_end = (ptr as usize)
        .checked_add(USER_CSTR_MAX_LEN)
        .ok_or(Errno::EFAULT)?
        .min(area_end);

    let mut cur = ptr as usize;
    let mut ret = String::new();

    while cur < max_end {
        let ch = unsafe { *(cur as *const u8) };
        if ch == 0 {
            return Ok(ret);
        }
        ret.push(ch as char);
        cur += 1;
    }

    Err(Errno::EFAULT)
}


pub fn copy_to_user<T: Copy>(dst: *mut T, src: *const T, len: usize) -> SysResult<usize> {
    if dst.is_null() || src.is_null() {
        return Err(Errno::EFAULT);
    }
    if len == 0 { return Ok(0); }

    // 检查地址是否合法
    let byte_len = len // 防止溢出
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    let end = (src as usize) // 防止溢出
        .checked_add(byte_len)
        .ok_or(Errno::EFAULT)?;
    let start_vpn = VirtAddr::from(src as usize).floor();
    let end_vpn = VirtAddr::from(end).ceil();
    let vpn_range = VPNRange::new(start_vpn, end_vpn);
    current_task()
        .expect("[kernel] current task is None.")
        .inner_exclusive_access()
        .memory_set
        .check_valid_user_vpn_range(vpn_range, MapPermission::WRITE)?;
    // 执行复制
    unsafe {
        let src_slice = core::slice::from_raw_parts(src, len);
        let dst_slice = core::slice::from_raw_parts_mut(dst, len);
        dst_slice.copy_from_slice(src_slice);
    }
    Ok(len)
}
