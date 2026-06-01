// os/src/mm.rs

//! ### 内存管理模块
//!
//! 实现虚拟地址空间
//!
//! 这部分内容繁多，建立了多层的抽象，隐含了很多深远的设计思想，需要好好消化

mod address;
mod frame_allocator;
mod heap_allocator;
mod memory_set;

use crate::arch::mm::{PTEFlags, PageTable, PageTableEntry};
use crate::config::{USER_ARG_MAX_COUNT, USER_CSTR_MAX_LEN}; // 该常量定义于 config/syscall.rs 中
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
pub use address::*;
use alloc::string::String;
use alloc::vec::Vec;
use frame_allocator::init_frame_allocator;
pub use frame_allocator::{FrameTracker, frame_alloc};
use heap_allocator::init_heap;
pub use memory_set::{KERNEL_SPACE, MapPermission, MemorySet};

/// 初始化内存管理，启用虚拟地址
pub fn init() {
    init_heap();
    init_frame_allocator();
    KERNEL_SPACE.lock().activate();
    // 注意此时已经启用了虚拟地址
}

/// 将 C 风格的字符串转换为 Rust 型字符串
pub fn copy_cstr_from_user(ptr: *const u8) -> SysResult<String> {
    if ptr.is_null() {
        return Err(Errno::EFAULT);
    }

    let start_vpn = VirtAddr::from(ptr as usize).floor();

    let vpn_range = current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_read(|memory_set| {
            // TODO[ABI-COMPAT]: 当前 C 字符串读取只允许字符串落在起始逻辑段内；
            // 后续需要支持跨相邻合法逻辑段的用户字符串。
            memory_set.check_valid_user_vpn(start_vpn, MapPermission::READ)
        })?;

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

pub fn extract_cstrings_from_user(mut ptr: *const usize) -> SysResult<Vec<String>> {
    let mut ret: Vec<String> = Vec::new();
    let mut count = 0;
    loop {
        if count > USER_ARG_MAX_COUNT {
            return Err(Errno::E2BIG); // 参数过多
        }

        let mut str_ptr: *const u8 = core::ptr::null();
        copy_from_user(&mut str_ptr as *mut *const u8, ptr as *const *const u8, 1)?;
        if str_ptr.is_null() {
            break;
        }
        ret.push(copy_cstr_from_user(str_ptr)?);

        count += 1;
        ptr = unsafe { ptr.add(1) };
    }

    Ok(ret)
}

/// 从用户空间拷贝数据到内核空间
///
/// 内部实现对数据有效性的检验
pub fn copy_from_user<T: Copy>(dst: *mut T, src: *const T, len: usize) -> SysResult<usize> {
    if dst.is_null() || src.is_null() {
        return Err(Errno::EFAULT);
    }
    if len == 0 {
        return Ok(0);
    }

    // 检验来源地址有效性
    check_user_readable(src, len)?;
    // 执行复制
    unsafe {
        let src_slice = core::slice::from_raw_parts(src, len);
        let dst_slice = core::slice::from_raw_parts_mut(dst, len);
        dst_slice.copy_from_slice(src_slice);
    }
    Ok(len)
}

/// 从内核空间拷贝数据到用户空间
///
/// 内部实现对数据有效性的检验
pub fn copy_to_user<T: Copy>(dst: *mut T, src: *const T, len: usize) -> SysResult<usize> {
    if dst.is_null() || src.is_null() {
        return Err(Errno::EFAULT);
    }
    if len == 0 {
        return Ok(0);
    }

    // 检验目标地址有效性
    check_user_writable(dst, len)?;
    // 执行复制
    unsafe {
        let src_slice = core::slice::from_raw_parts(src, len);
        let dst_slice = core::slice::from_raw_parts_mut(dst, len);
        dst_slice.copy_from_slice(src_slice);
    }
    Ok(len)
}

pub fn check_user_readable<T>(src: *const T, len: usize) -> SysResult {
    if src.is_null() {
        return Err(Errno::EFAULT);
    }
    let byte_len = len
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    check_user_buffer(src as usize, byte_len, MapPermission::READ)
}

pub fn check_user_writable<T>(dst: *mut T, len: usize) -> SysResult {
    if dst.is_null() {
        return Err(Errno::EFAULT);
    }
    let byte_len = len
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    check_user_buffer(dst as usize, byte_len, MapPermission::WRITE)
}

/// 检验数据段是否合法；检验数据段是否符合访问权限
///
/// 当前检验不支持跨逻辑段的数据
fn check_user_buffer(start: usize, byte_len: usize, perm: MapPermission) -> SysResult {
    // TODO[ABI-COMPAT]: 当前要求整个用户 buffer 完整落在同一个逻辑段内；
    // 后续需要按页/按逻辑段逐段检查，允许跨相邻且权限满足的逻辑段。
    if byte_len == 0 {
        return Ok(());
    }
    let end = start // 防止溢出
        .checked_add(byte_len)
        .ok_or(Errno::EFAULT)?;
    let start_vpn = VirtAddr::from(start).floor();
    let end_vpn = VirtAddr::from(end).ceil();
    let vpn_range = VPNRange::new(start_vpn, end_vpn);
    current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_write(|memory_set| {
            memory_set.check_valid_user_vpn_range(vpn_range.clone(), perm)?;
            memory_set.ensure_user_page_access(vpn_range, perm)
        })?;
    Ok(())
}
