// os/src/mm.rs

//! 内存管理公共入口与用户空间安全访问接口。
//!
//! 本模块重导出地址、frame、页表和 `MemorySet` 抽象，并负责 heap/frame 初始化顺序、
//! 用户 C 字符串与 argv/env 提取、copyin/copyout 以及跨页可访问性检查。syscall 层必须
//! 通过这些 helper 访问用户地址，不能把用户指针直接转换为 Rust 引用。
//!
//! copy helper 可能按规则处理 lazy page、COW 和普通缺页，所以调用方不能持有会被 fault
//! 路径再次获取的 MM/FS/task 锁。pathname、exec 单字符串、参数个数和聚合字节各有独立
//! 上限与 errno；扩大其中一个预算不能顺带放宽其他 ABI 边界。

mod address;
mod frame_allocator;
mod heap_allocator;
mod io_buffer;
mod memory_set;

use crate::arch::mm::{PTEFlags, PageTable, PageTableEntry};
use crate::config::{
    PAGE_SIZE, TRAMPOLINE, USER_ARG_MAX_BYTES, USER_ARG_MAX_COUNT, USER_ARG_STR_MAX_LEN,
    USER_CSTR_MAX_LEN,
};
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
pub use address::*;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use frame_allocator::init_frame_allocator;
pub use frame_allocator::{frame_alloc, FrameTracker};
use heap_allocator::init_heap;
pub use io_buffer::{drain_io_buffers, IoBufferKind, KernelIoBuffer};
pub(crate) use memory_set::{
    mmap_file_backing, overlay_shared_file_pages, punch_file_mappings, punch_shared_file_pages,
    protect_extended_file_mappings, shared_file_page_entry_count, truncate_file_mappings,
    truncate_shared_file_pages, update_shared_file_pages, writeback_file_pages, MmapBacking,
};
pub use memory_set::{MapPermission, MemorySet, PageFaultOutcome, KERNEL_SPACE};

static KERNEL_MMU_TOKEN: AtomicUsize = AtomicUsize::new(0);

pub fn free_frame_count() -> usize {
    frame_allocator::FRAME_ALLOCATOR.lock().free_frames()
}

pub fn heap_allocated() -> usize {
    heap_allocator::HEAP_ALLOCATOR.stats_alloc_user()
}

#[cfg(feature = "perf_counters")]
pub fn heap_perf_usage() -> (usize, usize, bool) {
    heap_allocator::HEAP_ALLOCATOR.perf_usage()
}

#[cfg(feature = "perf_counters")]
pub fn reset_heap_perf_peak() {
    heap_allocator::HEAP_ALLOCATOR.reset_perf_peak();
}

#[cfg(feature = "heap_magazine")]
pub fn heap_magazine_usage() -> (usize, usize) {
    heap_allocator::HEAP_ALLOCATOR.magazine_usage()
}

#[cfg(all(feature = "heap_magazine", feature = "perf_counters"))]
pub fn drain_heap_magazines() -> usize {
    heap_allocator::HEAP_ALLOCATOR.drain_magazines()
}

pub fn try_free_frame_count() -> Option<usize> {
    Some(frame_allocator::FRAME_ALLOCATOR.try_lock()?.free_frames())
}

pub fn try_heap_allocated() -> Option<usize> {
    heap_allocator::HEAP_ALLOCATOR.try_stats_alloc_user()
}

/// 初始化内存管理，启用虚拟地址
pub fn init() {
    #[cfg(target_arch = "loongarch64")]
    crate::arch::enable_boot_paging();
    let heap_reserved_end = init_heap();
    init_frame_allocator(heap_reserved_end);
    #[cfg(debug_assertions)]
    memory_set::run_split_self_tests();
    {
        let kernel_space = KERNEL_SPACE.lock();
        KERNEL_MMU_TOKEN.store(kernel_space.page_table.token(), Ordering::Release);
        kernel_space.activate();
    }
    #[cfg(target_arch = "loongarch64")]
    crate::arch::disable_low_direct_map();
    // 注意此时已经启用了虚拟地址
}

/// 在已经完成全局内存初始化的次 hart 上重新装载内核页表。
pub fn activate_kernel_space() {
    KERNEL_SPACE.lock().activate();
}

/// 不可变内核半区的根页表令牌，在次级核启动前发布。
/// 调度器 idle 上下文无需取得 KERNEL_SPACE 锁即可使用它。
pub fn kernel_mmu_token() -> usize {
    let token = KERNEL_MMU_TOKEN.load(Ordering::Acquire);
    assert_ne!(token, 0, "kernel MMU token is not initialized");
    token
}

/// 强制保证 idle 代码绝不在用户根页表上运行这一调度不变量。
/// 过期用户根会在最后一个任务退出后被回收，因此发生 SMP 迁移时，
/// 仅依赖 idle TaskContext 中保存的旧值并不安全。
#[cfg(target_arch = "loongarch64")]
pub fn ensure_kernel_space_active() {
    let token = kernel_mmu_token();
    if crate::arch::read_mmu_token() != token || crate::arch::register::mmu::read_pgdh() != token {
        crate::arch::write_mmu_token(token);
        crate::arch::sfence();
        crate::perf::local_sfence(1);
    }
}

/// 将 C 风格的字符串转换为 Rust 型字符串
pub fn copy_cstr_from_user(ptr: *const u8) -> SysResult<String> {
    copy_cstr_from_user_bounded(ptr, USER_CSTR_MAX_LEN, Errno::ENAMETOOLONG)
}

fn copy_cstr_from_user_bounded(
    ptr: *const u8,
    max_len: usize,
    too_long: Errno,
) -> SysResult<String> {
    if ptr.is_null() {
        return Err(Errno::EFAULT);
    }

    let mut ret = String::new();
    let mut offset = 0usize;
    let mut chunk = [0u8; 256];
    while offset < max_len {
        let cur = (ptr as usize).checked_add(offset).ok_or(Errno::EFAULT)?;
        let chunk_len = (PAGE_SIZE - VirtAddr::from(cur).page_offset())
            .min(max_len - offset)
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

    Err(too_long)
}

pub fn extract_cstrings_from_user(mut ptr: *const usize) -> SysResult<Vec<String>> {
    let mut ret: Vec<String> = Vec::new();
    let mut count = 0;
    let mut total_bytes = 0usize;
    loop {
        let mut str_ptr: *const u8 = core::ptr::null();
        copy_from_user(&mut str_ptr as *mut *const u8, ptr as *const *const u8, 1)?;
        if str_ptr.is_null() {
            break;
        }
        if count >= USER_ARG_MAX_COUNT {
            return Err(Errno::E2BIG); // 参数过多
        }
        // exec 字符串采用 Linux 更大的 MAX_ARG_STRLEN 上限；单串或总预算超限均报告 E2BIG。
        // 路径名调用者仍使用 USER_CSTR_MAX_LEN/ENAMETOOLONG。
        let string = copy_cstr_from_user_bounded(str_ptr, USER_ARG_STR_MAX_LEN, Errno::E2BIG)?;
        total_bytes = total_bytes
            .checked_add(string.len() + 1)
            .ok_or(Errno::E2BIG)?;
        if total_bytes > USER_ARG_MAX_BYTES {
            return Err(Errno::E2BIG);
        }
        ret.push(string);

        count += 1;
        let next = (ptr as usize)
            .checked_add(core::mem::size_of::<usize>())
            .ok_or(Errno::EFAULT)?;
        ptr = next as *const usize;
    }

    Ok(ret)
}

/// 从用户空间拷贝数据到内核空间
///
/// 内部实现对数据有效性的检验
pub fn copy_from_user<T: Copy>(dst: *mut T, src: *const T, len: usize) -> SysResult<usize> {
    if len == 0 {
        return Ok(0);
    }
    if dst.is_null() || src.is_null() {
        return Err(Errno::EFAULT);
    }

    let byte_len = len
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    if byte_len > isize::MAX as usize || (dst as usize).checked_add(byte_len).is_none() {
        return Err(Errno::EFAULT);
    }
    let dst_bytes = unsafe { core::slice::from_raw_parts_mut(dst as *mut u8, byte_len) };
    let started = crate::perf::now_ticks();
    let result = copy_user_bytes_to_kernel(src as usize, dst_bytes);
    crate::perf::copy_from_user(byte_len, crate::perf::elapsed_since(started));
    result?;
    Ok(len)
}

/// 从内核空间拷贝数据到用户空间
///
/// 内部实现对数据有效性的检验
pub fn copy_to_user<T: Copy>(dst: *mut T, src: *const T, len: usize) -> SysResult<usize> {
    if len == 0 {
        return Ok(0);
    }
    if dst.is_null() || src.is_null() {
        return Err(Errno::EFAULT);
    }

    let byte_len = len
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::EFAULT)?;
    if byte_len > isize::MAX as usize || (src as usize).checked_add(byte_len).is_none() {
        return Err(Errno::EFAULT);
    }
    let src_bytes = unsafe { core::slice::from_raw_parts(src as *const u8, byte_len) };
    let started = crate::perf::now_ticks();
    let result = copy_kernel_bytes_to_user(dst as usize, src_bytes);
    crate::perf::copy_to_user(byte_len, crate::perf::elapsed_since(started));
    result?;
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
    let vpn_range = checked_user_byte_range(user_start, dst.len())?;
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
    let vpn_range = checked_user_byte_range(user_start, src.len())?;
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

/// 在不解析惰性页面的前提下，读取一个对齐的 futex 字。
///
/// 调用者必须先在所有全局自旋锁之外执行 [`check_user_readable`]。
/// 第二次读取只取得地址空间读锁并翻译已存在的 PTE，因此持有 futex 队列锁时
/// 不会分配页面或处理缺页。
pub fn read_user_u32_nofault(src: *const u32) -> SysResult<u32> {
    let addr = src as usize;
    if src.is_null() {
        return Err(Errno::EFAULT);
    }
    if addr % core::mem::align_of::<u32>() != 0 {
        return Err(Errno::EINVAL);
    }

    let va = VirtAddr::from(addr);
    let vpn = va.floor();
    let page_offset = va.page_offset();
    if page_offset > PAGE_SIZE - core::mem::size_of::<u32>() {
        return Err(Errno::EFAULT);
    }
    let task = current_task().ok_or(Errno::ESRCH)?;
    task.op_memory_set_read(|memory_set| {
        memory_set.check_user_access_range(
            VPNRange::new(vpn, VirtPageNum::from(usize::from(vpn) + 1)),
            MapPermission::READ,
        )?;
        let pte = memory_set.page_table.translate(vpn).ok_or(Errno::EFAULT)?;
        if !pte.is_valid() {
            return Err(Errno::EFAULT);
        }
        let bytes = &pte.ppn().get_bytes_array()[page_offset..page_offset + 4];
        let mut value = [0; core::mem::size_of::<u32>()];
        value.copy_from_slice(bytes);
        Ok(u32::from_ne_bytes(value))
    })
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
    let vpn_range = checked_user_byte_range(start, byte_len)?;
    current_task()
        .expect("[kernel] current task is None.")
        .op_memory_set_write(|memory_set| {
            memory_set.check_user_access_range(vpn_range.clone(), perm)?;
            memory_set.ensure_user_page_access(vpn_range, perm)
        })?;
    Ok(())
}

fn checked_user_byte_range(start: usize, byte_len: usize) -> SysResult<VPNRange> {
    if byte_len == 0 {
        return Ok(VPNRange::new(
            VirtAddr::from(start).floor(),
            VirtAddr::from(start).floor(),
        ));
    }
    let end = start.checked_add(byte_len).ok_or(Errno::EFAULT)?;
    let rounded_end = end.checked_add(PAGE_SIZE - 1).ok_or(Errno::EFAULT)? & !(PAGE_SIZE - 1);
    if start == 0 || start >= TRAMPOLINE || rounded_end > TRAMPOLINE {
        return Err(Errno::EFAULT);
    }
    Ok(VPNRange::new(
        VirtAddr::from(start).floor(),
        VirtAddr::from(rounded_end).floor(),
    ))
}
