// os/src/syscall/mm.rs

use super::{Errno, SysResult};
use crate::config::{MMAP_MIN_ADDR, PAGE_SIZE};
use crate::mm::{MapPermission, MmapBacking, VPNRange, VirtAddr, mmap_file_backing};
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

        let old_brk = memory_set.brk;
        let old_end = VirtAddr::from(old_brk).ceil();
        let new_end = VirtAddr::from(addr).ceil();

        if addr < old_brk {
            if new_end < old_end {
                // Linux 允许用户用 munmap() 打洞或切碎 brk 区间；收缩 brk 时应删除
                // 新旧堆顶之间所有重叠映射，而不是假设堆始终是一段连续 VMA。
                memory_set.remove_area_with_overlap_range(VPNRange::new(new_end, old_end))?;
            }
        } else if new_end > old_end {
            if new_end > VirtAddr::from(MMAP_MIN_ADDR).floor() {
                return Ok(old_brk);
            }

            match memory_set.remap_writable_area_lazy_from_end(old_end, new_end) {
                Ok(()) => {}
                Err(Errno::EINVAL) => {
                    if memory_set
                        .ensure_private_writable_anonymous_range(VPNRange::new(old_end, new_end))
                        .is_err()
                    {
                        return Ok(old_brk);
                    }
                }
                Err(_) => return Ok(old_brk),
            }
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
    let raw_flags = flags as u32;
    let shared_validate =
        raw_flags & MMAPFLAGS::MAP_SHARED_VALIDATE.bits() == MMAPFLAGS::MAP_SHARED_VALIDATE.bits();
    if shared_validate && raw_flags & !MMAPFLAGS::all().bits() != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    let flags = MMAPFLAGS::from_bits_truncate(raw_flags);
    let has_shared = flags.contains(MMAPFLAGS::MAP_SHARED) || shared_validate;
    let has_private = flags.contains(MMAPFLAGS::MAP_PRIVATE) && !shared_validate;
    if has_shared == has_private {
        return Err(Errno::EINVAL);
    }
    let replace = flags.contains(MMAPFLAGS::MAP_FIXED);
    let noreplace = flags.contains(MMAPFLAGS::MAP_FIXED_NOREPLACE);
    let locked = flags.contains(MMAPFLAGS::MAP_LOCKED);
    let fixed = replace || noreplace;
    if fixed && (_addr % PAGE_SIZE != 0 || _addr == 0) {
        return Err(Errno::EINVAL);
    }
    let fixed_addr = fixed.then_some(_addr);

    let mut permission = MapPermission::from(prot);
    permission |= MapPermission::USER;

    let task = current_task().expect("[kernel] current task is None.");
    if flags.contains(MMAPFLAGS::MAP_ANONYMOUS) {
        // 匿名映射忽略 fd，但 offset 必须为 0。
        if offset != 0 {
            return Err(Errno::EINVAL);
        }
        task.op_memory_set_write(|memory_set| {
            let backing = if has_shared {
                MmapBacking::SharedAnonymous
            } else {
                MmapBacking::LazyAnonymous
            };
            let start = memory_set.mmap_area(
                fixed_addr, map_len, permission, replace, noreplace, locked, backing,
            )?;
            memory_set.flush_tlb();
            Ok(start)
        })
    } else {
        // 文件映射：当前是假实现，只把文件内容读入一份私有物理页拷贝。
        if fd < 0 {
            return Err(Errno::EBADF);
        }
        if offset % PAGE_SIZE != 0 {
            return Err(Errno::EINVAL);
        }
        let file = task.get_fd_entry(fd as usize)?.get_file();
        if !file.readable() {
            return Err(Errno::EACCES);
        }
        let backing = mmap_file_backing(file, offset, len, map_len, has_shared)?;

        task.op_memory_set_write(|memory_set| {
            let start = memory_set.mmap_area(
                fixed_addr, map_len, permission, replace, noreplace, locked, backing,
            )?;
            memory_set.flush_tlb();
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
    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_write(|memory_set| {
        memory_set.munmap_range(addr, map_len)?;
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

fn mlock_vpn_range(addr: usize, len: usize) -> SysResult<Option<(VPNRange, usize)>> {
    if len == 0 {
        return Ok(None);
    }
    let end = addr.checked_add(len).ok_or(Errno::EINVAL)?;
    let start_vpn = VirtAddr::from(addr).floor();
    let end_vpn = VirtAddr::from(end).ceil();
    let locked_len = end_vpn.0.saturating_sub(start_vpn.0) * PAGE_SIZE;
    Ok(Some((VPNRange::new(start_vpn, end_vpn), locked_len)))
}

/// 系统调用 sys_mlock
///
/// 当前内核没有换出机制，锁页成功不需要额外状态；仍按 Linux ABI 校验地址区间和
/// RLIMIT_MEMLOCK，并在失败时返回 ENOMEM/EPERM。
pub fn sys_mlock(addr: usize, len: usize) -> SysResult<usize> {
    let Some((vpn_range, locked_len)) = mlock_vpn_range(addr, len)? else {
        return Ok(0);
    };

    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_read(|memory_set| memory_set.check_user_mapped_range(vpn_range))
        .map_err(|err| {
            if err == Errno::EFAULT {
                Errno::ENOMEM
            } else {
                err
            }
        })?;

    if task.euid() != 0 {
        let limit = task.memlock_limit().0;
        if limit == 0 {
            return Err(Errno::EPERM);
        }
        if locked_len > limit {
            return Err(Errno::ENOMEM);
        }
    }

    Ok(0)
}

/// 系统调用 sys_munlock
pub fn sys_munlock(addr: usize, len: usize) -> SysResult<usize> {
    let Some((vpn_range, _)) = mlock_vpn_range(addr, len)? else {
        return Ok(0);
    };
    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_read(|memory_set| memory_set.check_user_mapped_range(vpn_range))
        .map_err(|err| {
            if err == Errno::EFAULT {
                Errno::ENOMEM
            } else {
                err
            }
        })?;
    Ok(0)
}

/// 系统调用 sys_madvise
///
/// 当前内核没有页回收器，也没有 per-VMA 行为标志；这里先按 Linux ABI 接受 libc
/// 常见 advice，尤其是 glibc pthread 栈缓存路径使用的 MADV_DONTNEED/MADV_FREE。
pub fn sys_madvise(addr: usize, len: usize, advice: i32) -> SysResult<usize> {
    if addr % PAGE_SIZE != 0 {
        return Err(Errno::EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    let _end = addr.checked_add(len).ok_or(Errno::EINVAL)?;

    match advice {
        0  // MADV_NORMAL
        | 1  // MADV_RANDOM
        | 2  // MADV_SEQUENTIAL
        | 3  // MADV_WILLNEED
        | 4  // MADV_DONTNEED
        | 8  // MADV_FREE
        | 10 // MADV_DONTFORK
        | 11 // MADV_DOFORK
        | 12 // MADV_MERGEABLE
        | 13 // MADV_UNMERGEABLE
        | 14 // MADV_HUGEPAGE
        | 15 // MADV_NOHUGEPAGE
        | 16 // MADV_DONTDUMP
        | 17 // MADV_DODUMP
        | 18 // MADV_WIPEONFORK
        | 19 // MADV_KEEPONFORK
        | 20 // MADV_COLD
        | 21 // MADV_PAGEOUT
        | 22 // MADV_POPULATE_READ
        | 23 // MADV_POPULATE_WRITE
        | 25 // MADV_COLLAPSE
        | 26 // MADV_GUARD_INSTALL
        | 27 => Ok(0), // MADV_GUARD_REMOVE
        _ => Err(Errno::EINVAL),
    }
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
        /// MAP_SHARED_VALIDATE 共享映射，并要求内核拒绝未知 flag。
        const MAP_SHARED_VALIDATE = 0x03;
        /// MAP_FIXED 固定映射，固定映射到addr
        const MAP_FIXED = 1 << 4;
        /// MAP_ANONYMOUS 匿名映射，需要fd为 -1, offset为 0
        const MAP_ANONYMOUS = 1 << 5;
        /// MAP_GROWSDOWN 栈类映射。当前实现不做自动增长，只接受该 flag。
        const MAP_GROWSDOWN = 1 << 8;
        /// MAP_DENYWRITE 历史兼容 flag，当前忽略。
        const MAP_DENYWRITE = 1 << 11;
        /// MAP_LOCKED 当前忽略。
        const MAP_LOCKED = 1 << 13;
        /// MAP_NORESERVE 当前忽略。
        const MAP_NORESERVE = 1 << 14;
        /// MAP_POPULATE 当前忽略。
        const MAP_POPULATE = 1 << 15;
        /// MAP_STACK 当前忽略。
        const MAP_STACK = 1 << 17;
        /// MAP_FIXED_NOREPLACE 固定映射，但不能覆盖已有映射。
        const MAP_FIXED_NOREPLACE = 1 << 20;
    }
}
