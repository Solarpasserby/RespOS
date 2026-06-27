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
use crate::config::{PAGE_SIZE, USER_ARG_MAX_COUNT, USER_CSTR_MAX_LEN};
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
pub use address::*;
use alloc::string::String;
use alloc::vec::Vec;
use frame_allocator::init_frame_allocator;
pub use frame_allocator::{FrameTracker, frame_alloc};
use heap_allocator::init_heap;
pub use memory_set::{KERNEL_SPACE, MapPermission, MemorySet};
pub(crate) use memory_set::{MmapBacking, mmap_file_backing};

pub fn free_frame_count() -> usize {
    frame_allocator::FRAME_ALLOCATOR.lock().free_frames()
}

pub fn heap_allocated() -> usize {
    heap_allocator::HEAP_ALLOCATOR.lock().stats_alloc_user()
}

pub fn try_free_frame_count() -> Option<usize> {
    Some(frame_allocator::FRAME_ALLOCATOR.try_lock()?.free_frames())
}

pub fn try_heap_allocated() -> Option<usize> {
    Some(
        heap_allocator::HEAP_ALLOCATOR
            .try_lock()?
            .stats_alloc_user(),
    )
}

/// 初始化内存管理，启用虚拟地址
pub fn init() {
    #[cfg(target_arch = "loongarch64")]
    crate::arch::enable_boot_paging();
    init_heap();
    init_frame_allocator();
    KERNEL_SPACE.lock().activate();
    #[cfg(target_arch = "loongarch64")]
    crate::arch::disable_low_direct_map();
    // 注意此时已经启用了虚拟地址
}

/// 将 C 风格的字符串转换为 Rust 型字符串
pub fn copy_cstr_from_user(ptr: *const u8) -> SysResult<String> {
    if ptr.is_null() {
        return Err(Errno::EFAULT);
    }

    let mut ret = String::new();
    let mut offset = 0usize;
    let mut chunk = [0u8; 256];
    while offset < USER_CSTR_MAX_LEN {
        let cur = (ptr as usize).checked_add(offset).ok_or(Errno::EFAULT)?;
        let chunk_len = (PAGE_SIZE - VirtAddr::from(cur).page_offset())
            .min(USER_CSTR_MAX_LEN - offset)
            .min(chunk.len());
        copy_from_user(chunk.as_mut_ptr(), cur as *const u8, chunk_len)?;
        for &ch in &chunk[..chunk_len] {
            if ch == 0 {
                return Ok(ret);
            }
            ret.push(ch as char);
            offset += 1;
        }
    }

    Err(Errno::ENAMETOOLONG)
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

    let byte_len = len
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    let dst_bytes = unsafe { core::slice::from_raw_parts_mut(dst as *mut u8, byte_len) };
    copy_user_bytes_to_kernel(src as usize, dst_bytes)?;
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

    let byte_len = len
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    let src_bytes = unsafe { core::slice::from_raw_parts(src as *const u8, byte_len) };
    copy_kernel_bytes_to_user(dst as usize, src_bytes)?;
    Ok(len)
}

/// 用户→内核拷贝：逐页通过页表翻译到物理地址后复制。
///
/// 不能直接解引用用户虚拟地址：用户页可能是惰性分配的（匿名 mmap），
/// 虚拟地址上没有映射时解引用会触发 kernel page fault。
/// 通过页表 PTE → ppn → get_bytes_array 读写物理页帧，
/// 绕过了虚拟地址的映射延迟问题。
fn copy_user_bytes_to_kernel(user_start: usize, dst: &mut [u8]) -> SysResult {
    let mut copied = 0usize;
    let mut cur = user_start;
    let end = user_start.checked_add(dst.len()).ok_or(Errno::EFAULT)?;
    let vpn_range = VPNRange::new(
        VirtAddr::from(user_start).floor(),
        VirtAddr::from(end).ceil(),
    );
    current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_write(|memory_set| {
            memory_set.check_user_access_range(vpn_range.clone(), MapPermission::READ)?;
            memory_set.ensure_user_page_access(vpn_range, MapPermission::READ)?;
            while copied < dst.len() {
                let va = VirtAddr::from(cur);
                let vpn = va.floor();
                let page_offset = va.page_offset();
                // 每次最多拷贝到当前页末尾，超过则下一轮切到下一页
                let copy_len = (PAGE_SIZE - page_offset).min(dst.len() - copied);
                let pte = memory_set.page_table.translate(vpn).ok_or(Errno::EFAULT)?;
                if !pte.is_valid() {
                    return Err(Errno::EFAULT);
                }
                let src = &pte.ppn().get_bytes_array()[page_offset..page_offset + copy_len];
                dst[copied..copied + copy_len].copy_from_slice(src);
                copied += copy_len;
                cur = cur.checked_add(copy_len).ok_or(Errno::EFAULT)?;
            }
            Ok(())
        })
}

/// 内核→用户拷贝：逐页通过页表翻译到物理地址后写入。
///
/// 与 copy_user_bytes_to_kernel 对称，写入方向相反。
/// 同样通过物理页帧写入，避免直接解引用用户虚拟地址。
fn copy_kernel_bytes_to_user(user_start: usize, src: &[u8]) -> SysResult {
    let mut copied = 0usize;
    let mut cur = user_start;
    let end = user_start.checked_add(src.len()).ok_or(Errno::EFAULT)?;
    let vpn_range = VPNRange::new(
        VirtAddr::from(user_start).floor(),
        VirtAddr::from(end).ceil(),
    );
    current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_write(|memory_set| {
            memory_set.check_user_access_range(vpn_range.clone(), MapPermission::WRITE)?;
            memory_set.ensure_user_page_access(vpn_range, MapPermission::WRITE)?;
            while copied < src.len() {
                let va = VirtAddr::from(cur);
                let vpn = va.floor();
                let page_offset = va.page_offset();
                let copy_len = (PAGE_SIZE - page_offset).min(src.len() - copied);
                let pte = memory_set.page_table.translate(vpn).ok_or(Errno::EFAULT)?;
                if !pte.is_valid() {
                    return Err(Errno::EFAULT);
                }
                let dst = &mut pte.ppn().get_bytes_array()[page_offset..page_offset + copy_len];
                dst.copy_from_slice(&src[copied..copied + copy_len]);
                copied += copy_len;
                cur = cur.checked_add(copy_len).ok_or(Errno::EFAULT)?;
            }
            Ok(())
        })
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
/// 允许跨过多个相邻且权限满足的用户逻辑段。
fn check_user_buffer(start: usize, byte_len: usize, perm: MapPermission) -> SysResult {
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
            memory_set.check_user_access_range(vpn_range.clone(), perm)?;
            memory_set.ensure_user_page_access(vpn_range, perm)
        })?;
    Ok(())
}
