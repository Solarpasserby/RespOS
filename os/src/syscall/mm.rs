// os/src/syscall/mm.rs

use super::{Errno, SysResult};
use crate::config::{MMAP_MAX_ADDR, MMAP_MIN_ADDR, PAGE_SIZE};
use crate::mm::{MapPermission, VPNRange, VirtAddr};
use crate::task::current_task;
use bitflags::bitflags;

/// 系统调用 sys-brk
pub fn sys_brk(addr: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_write(|memory_set| {
        // addr 为 0 获取当前堆顶
        if addr == 0 || addr < memory_set.heap_bottom || addr == memory_set.brk {
            return Ok(memory_set.brk);
        }

        let heap_start = VirtAddr::from(memory_set.heap_bottom);
        let new_end = VirtAddr::from(addr);

        if addr == memory_set.heap_bottom {
            memory_set.remove_area_with_start_vpn(heap_start.floor())?;
        } else if memory_set.brk == memory_set.heap_bottom {
            // 惰性分配
            memory_set.insert_framed_area_va_lazy(
                heap_start,
                new_end,
                MapPermission::READ | MapPermission::WRITE | MapPermission::USER,
            );
        } else {
            // 惰性分配，惰态修改
            memory_set.remap_area_lazy(heap_start.floor(), new_end.ceil())?;
        }

        memory_set.brk = addr;
        memory_set.flush_tlb(); // 修改地址空间后刷新页表
        Ok(memory_set.brk)
    })
}

/// 系统调用 sys-mmap
/// TODO: 这个 mmap 逻辑有些过于复杂了，目前只做最基础实现
pub fn sys_mmap(
    _addr: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: isize,
    offset: usize,
) -> SysResult<usize> {
    if len == 0 {
        return Err(Errno::EINVAL);
    }
    let map_len = len.checked_add(PAGE_SIZE - 1).ok_or(Errno::ENOMEM)? & !(PAGE_SIZE - 1);

    let prot = MMapProt::from_bits(prot as u32).ok_or(Errno::EINVAL)?;
    let flags = MMAPFLAGS::from_bits(flags as u32).ok_or(Errno::EINVAL)?;
    let has_shared = flags.contains(MMAPFLAGS::MAP_SHARED);
    let has_private = flags.contains(MMAPFLAGS::MAP_PRIVATE);
    if has_shared == has_private || flags.contains(MMAPFLAGS::MAP_FIXED) {
        return Err(Errno::EINVAL);
    }

    let mut permission = MapPermission::from(prot);
    permission |= MapPermission::USER;

    let task = current_task().expect("[kernel] current task is None.");
    if flags.contains(MMAPFLAGS::MAP_ANONYMOUS) {
        // 匿名映射限制 fd 为 -1，offset 为 0
        if fd != -1 || offset != 0 {
            return Err(Errno::EINVAL);
        }
        task.op_memory_set_write(|memory_set| {
            // start 可以保证是页对齐的
            let start = memory_set.mmap_start;
            let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
            if end > MMAP_MAX_ADDR {
                return Err(Errno::ENOMEM);
            }
            // 惰性分配
            memory_set.insert_framed_area_va_lazy(
                VirtAddr::from(start),
                VirtAddr::from(end),
                permission,
            );
            memory_set.mmap_start = end;
            memory_set.flush_tlb();
            Ok(start)
        })
    } else {
        // 文件映射：当前是假实现，只把文件内容读入一份私有物理页拷贝。
        if fd < 0 || offset % PAGE_SIZE != 0 {
            return Err(Errno::EINVAL);
        }
        let file = task.get_fd_entry(fd as usize)?.get_file();
        if !file.readable() {
            return Err(Errno::EACCES);
        }

        task.op_memory_set_write(|memory_set| {
            let start = memory_set.mmap_start;
            let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
            if end > MMAP_MAX_ADDR {
                return Err(Errno::ENOMEM);
            }
            memory_set.insert_framed_area_va(
                VirtAddr::from(start),
                VirtAddr::from(end),
                permission,
            );
            memory_set.mmap_start = end;
            memory_set.flush_tlb();

            let buf = unsafe { core::slice::from_raw_parts_mut(start as *mut u8, map_len) };
            buf.fill(0);

            // 没有复制文件内容，仅仅是模拟正常情况下的报错
            let origin_offset = file.get_offset();
            file.seek(offset as isize)?;
            let read_result = file.read(&mut buf[..len]);
            let restore_result = file.seek(origin_offset as isize);
            read_result?;
            restore_result?;
            Ok(start)
        })
    }
}

/// 系统调用 sys-munmap
/// TODO: 同样目前只做了做基础实现
pub fn sys_munmap(addr: usize, len: usize) -> SysResult<usize> {
    if addr % PAGE_SIZE != 0 || len == 0 || addr < MMAP_MIN_ADDR {
        return Err(Errno::EINVAL);
    }
    let map_len = len.checked_add(PAGE_SIZE - 1).ok_or(Errno::EINVAL)? & !(PAGE_SIZE - 1);
    let end = addr.checked_add(map_len).ok_or(Errno::EINVAL)?;
    if end > MMAP_MAX_ADDR {
        return Err(Errno::ENOMEM);
    }
    let start_vpn = VirtAddr::from(addr).floor();
    let end_vpn = VirtAddr::from(end).floor();
    let unmap_vpn_range = VPNRange::new(start_vpn, end_vpn);
    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_write(|memory_set| {
        memory_set.remove_area_with_overlap_range(unmap_vpn_range)?;
        memory_set.flush_tlb();
        Ok(0)
    })
}

/// 系统调用 sys_mprotect
///
/// 修改指定地址范围的页表权限 (PROT_READ / PROT_WRITE / PROT_EXEC)。
/// addr 必须页对齐, len 向上取整到页边界。
pub fn sys_mprotect(addr: usize, len: usize, prot: u32) -> SysResult<usize> {
    if addr % PAGE_SIZE != 0 || len == 0 {
        return Err(Errno::EINVAL);
    }

    let prot = MMapProt::from_bits(prot).ok_or(Errno::EINVAL)?;
    let map_len = len.checked_add(PAGE_SIZE - 1).ok_or(Errno::EINVAL)? & !(PAGE_SIZE - 1);
    let end = addr.checked_add(map_len).ok_or(Errno::EINVAL)?;
    let start_vpn = VirtAddr::from(addr).floor();
    let end_vpn = VirtAddr::from(end).floor();
    let remap_vpn_range = VPNRange::new(start_vpn, end_vpn);
    let map_perm = MapPermission::from(prot);

    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_write(|memory_set| {
        memory_set.remap_area_with_overlap_range(remap_vpn_range, map_perm)?;
        memory_set.flush_tlb();
        Ok(0)
    })
}

bitflags! {
    pub struct MMapProt: u32 {
        // 可读
        const PROT_READ  = 1 << 0;
        // 可写
        const PROT_WRITE = 1 << 1;
        // 可执行
        const PROT_EXEC  = 1 << 2;
    }
}

impl From<MMapProt> for MapPermission {
    fn from(prot: MMapProt) -> Self {
        let mut map_permission = MapPermission::from_bits(0).unwrap();
        if prot.contains(MMapProt::PROT_READ) {
            map_permission |= MapPermission::READ;
        }
        if prot.contains(MMapProt::PROT_WRITE) {
            map_permission |= MapPermission::WRITE;
        }
        if prot.contains(MMapProt::PROT_EXEC) {
            map_permission |= MapPermission::EXECUTE;
        }
        map_permission
    }
}

bitflags! {
    /// 决定映射区域对映射了相同区域的进程是否可见
    pub struct MMAPFLAGS: u32 {
        /// MAP_SHARED 共享映射
        const MAP_SHARED = 1 << 0;
        /// MAP_PRIVATE 私有映射
        const MAP_PRIVATE = 1 << 1;
        /// MAP_FIXED 固定映射，固定映射到addr
        const MAP_FIXED = 1 << 4;
        /// MAP_ANONYMOUS 匿名映射，需要fd为 -1, offset为 0
        const MAP_ANONYMOUS = 1 << 5;
    }
}
