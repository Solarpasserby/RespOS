// os/src/mm/memory_set.rs

use super::address::{PhysPageNum, StepByOne, VPNRange, VirtAddr, VirtPageNum};
use super::frame_allocator::{FrameTracker, frame_alloc};
use super::{PTEFlags, PageTable, PageTableEntry};
use crate::arch::{sfence, write_mmu_token};
use crate::config::{
    CLK_TCK, DL_INTERP_OFFSET, KERNEL_BASE, KERNEL_STACK_SIZE, MEMORY_END, MMAP_MAX_ADDR,
    MMAP_MIN_ADDR, PAGE_SIZE, PAGE_SIZE_BITS, TRAMPOLINE, TRAMPOLINE_CODE, USER_STACK_SIZE,
    VIRTIO_MMIO,
};
use crate::syscall::{Errno, SysResult};
use crate::task::{
    AT_BASE, AT_CLKTCK, AT_EGID, AT_ENTRY, AT_EUID, AT_GID, AT_PAGESZ, AT_PHDR, AT_PHENT, AT_PHNUM,
    AT_UID, AuxHeader,
};
use crate::trap::PageFaultCause;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use lazy_static::lazy_static;
use spin::Mutex;

unsafe extern "C" {
    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();
    fn sbss_with_stack();
    fn ebss();
    fn ekernel();
}

lazy_static! {
    /// 内核地址空间，内核地址空间在内核初始化后创建
    ///
    /// 内核采用恒等映射，因而开启虚拟地址后访问内核空间的地址不变
    ///
    /// 由于内核空间被所有用户空间共享，所以使用 `Arc` 来实现共享，使用 `Mutex` 来实现内部可变性
    pub static ref KERNEL_SPACE: Arc<Mutex<MemorySet>> =
        Arc::new(Mutex::new(MemorySet::new_kernel()));
}

/// 地址空间
///
/// 一系列有关联的逻辑段 [`MapArea`]，地址不一定连续
pub struct MemorySet {
    // 堆分配
    pub brk: usize,
    pub heap_bottom: usize,
    // mmap 起始地址
    pub mmap_start: usize,
    // 页表和各逻辑段
    pub page_table: PageTable,
    areas: Vec<MapArea>,
}

impl MemorySet {
    /// 将一段空的逻辑段加入地址空间，在内部完成映射关系的建立
    fn push_empty_map_area(
        &mut self,
        mut map_area: MapArea,
        data: Option<&[u8]>,
        data_offset: usize,
    ) {
        map_area
            .map(&mut self.page_table)
            .expect("failed to map area");
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data, data_offset);
        }
        self.areas.push(map_area); // 转移所有权
    }
    /// 惰性添加逻辑段——只记录虚拟地址范围，不分配物理页不建立映射
    fn push_map_area_lazy(&mut self, map_area: MapArea) {
        self.areas.push(map_area);
    }

    /// 对外暴露的添加内核栈段的接口
    pub fn insert_stack_area(&mut self, stack_top: usize) {
        self.push_empty_map_area(
            MapArea::new(
                (stack_top - KERNEL_STACK_SIZE).into(),
                stack_top.into(),
                MapType::Framed,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None,
            0,
        )
    }
    /// 对外暴露的删除内核栈段的接口
    pub fn remove_stack_area(&mut self, stack_top: usize) {
        let stack_bottom_va = VirtAddr::from(stack_top - KERNEL_STACK_SIZE);
        let stack_bottom_vpn = VirtPageNum::from(stack_bottom_va);
        if let Some((idx, area)) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| area.vpn_range.get_start() == stack_bottom_vpn)
        {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
        }
    }
    /// 插入已有逻辑段，由调用者保证插入逻辑段没有冲突
    pub fn insert_framed_area_va(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
    ) {
        self.push_empty_map_area(
            MapArea::new(start_va, end_va, MapType::Framed, map_perm),
            None,
            0,
        );
    }

    /// 将 sigreturn 跳板页映射到用户地址空间（TRAMPOLINE 虚拟地址）。
    ///
    /// 用户态信号处理函数返回后，会跳转到该页执行架构相关的 sigreturn
    /// 系统调用入口代码。每个用户进程都需要此映射。
    pub fn map_trampoline(&mut self) {
        // 在用户空间分配一页并映射
        self.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(TRAMPOLINE),
                VirtAddr::from(TRAMPOLINE + PAGE_SIZE),
                MapType::Framed,
                MapPermission::READ | MapPermission::EXECUTE | MapPermission::USER,
            ),
            Some(TRAMPOLINE_CODE),
            0,
        );
    }

    /// 惰性插入逻辑段，只预留虚拟地址空间，不分配物理页（由 page fault handler 按需分配）
    pub fn insert_framed_area_va_lazy(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
    ) {
        self.push_map_area_lazy(MapArea::new(start_va, end_va, MapType::Framed, map_perm));
    }
    /// 根据首虚拟页号删除对应逻辑段
    pub fn remove_area_with_start_vpn(&mut self, vpn_start: VirtPageNum) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| area.vpn_range.get_start() == vpn_start)
            .ok_or(Errno::EINVAL)?;

        area.unmap(&mut self.page_table);
        self.areas.remove(idx);
        Ok(())
    }

    /// 删除与给定虚拟页区间重叠的映射，必要时裁剪或切分原逻辑段
    pub fn remove_area_with_overlap_range(&mut self, vpn_range: VPNRange) -> SysResult {
        let mut idx = 0;
        while idx < self.areas.len() {
            if !self.areas[idx].vpn_range.intersect_with(&vpn_range) {
                idx += 1;
                continue;
            }

            let area_start = self.areas[idx].vpn_range.get_start();
            let area_end = self.areas[idx].vpn_range.get_end();
            let remove_start = vpn_range.get_start();
            let remove_end = vpn_range.get_end();
            let overlap_start = area_start.max(remove_start);
            let overlap_end = area_end.min(remove_end);

            let new_right_area = {
                let area = &mut self.areas[idx];
                for vpn in VPNRange::new(overlap_start, overlap_end) {
                    area.unmap_one(&mut self.page_table, vpn);
                }

                if overlap_start == area_start && overlap_end == area_end {
                    None
                } else if overlap_start == area_start {
                    area.vpn_range = VPNRange::new(overlap_end, area_end);
                    None
                } else if overlap_end == area_end {
                    area.vpn_range = VPNRange::new(area_start, overlap_start);
                    None
                } else {
                    let right_frames = area.data_frames.split_off(&overlap_end);
                    let right_area = MapArea {
                        vpn_range: VPNRange::new(overlap_end, area_end),
                        data_frames: right_frames,
                        map_type: area.map_type,
                        map_perm: area.map_perm,
                    };
                    area.vpn_range = VPNRange::new(area_start, overlap_start);
                    Some(right_area)
                }
            };

            if overlap_start == area_start && overlap_end == area_end {
                self.areas.remove(idx);
            } else {
                idx += 1;
            }

            if let Some(right_area) = new_right_area {
                self.areas.push(right_area);
            }
        }
        Ok(())
    }

    /// 判断给定虚拟页区间是否与当前任一逻辑段重叠。
    pub fn area_intersects(&self, vpn_range: &VPNRange) -> bool {
        self.areas
            .iter()
            .any(|area| area.vpn_range.intersect_with(vpn_range))
    }

    /// 按 mmap 语义选择一段地址并插入匿名懒分配区域。
    pub fn mmap_lazy_anonymous(
        &mut self,
        addr: Option<usize>,
        map_len: usize,
        map_perm: MapPermission,
        replace: bool,
        noreplace: bool,
    ) -> SysResult<usize> {
        let start = self.choose_mmap_start(addr, map_len)?;
        let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
        if end > MMAP_MAX_ADDR {
            return Err(Errno::ENOMEM);
        }

        let vpn_range = VPNRange::new(VirtAddr::from(start).floor(), VirtAddr::from(end).ceil());
        if noreplace && self.area_intersects(&vpn_range) {
            return Err(Errno::EEXIST);
        }
        if replace {
            self.remove_area_with_overlap_range(vpn_range)?;
        }

        self.insert_framed_area_va_lazy(VirtAddr::from(start), VirtAddr::from(end), map_perm);
        if addr.is_none() {
            self.mmap_start = end;
        }
        Ok(start)
    }

    /// 按 mmap 语义选择一段地址并插入已分配物理页的区域。
    pub fn mmap_framed(
        &mut self,
        addr: Option<usize>,
        map_len: usize,
        map_perm: MapPermission,
        replace: bool,
        noreplace: bool,
    ) -> SysResult<usize> {
        let start = self.choose_mmap_start(addr, map_len)?;
        let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
        if end > MMAP_MAX_ADDR {
            return Err(Errno::ENOMEM);
        }

        let vpn_range = VPNRange::new(VirtAddr::from(start).floor(), VirtAddr::from(end).ceil());
        if noreplace && self.area_intersects(&vpn_range) {
            return Err(Errno::EEXIST);
        }
        if replace {
            self.remove_area_with_overlap_range(vpn_range)?;
        }

        self.insert_framed_area_va(VirtAddr::from(start), VirtAddr::from(end), map_perm);
        if addr.is_none() {
            self.mmap_start = end;
        }
        Ok(start)
    }

    /// 将内核缓冲区写入已经映射好的用户虚拟地址范围。
    ///
    /// 该函数通过页表找到物理页后写内核直映地址，不受用户页 PTE 的
    /// 读写权限影响，适合用于 `mmap` 初始化只读文件页。
    pub fn write_bytes_to_mapped_range(&mut self, start: usize, data: &[u8]) -> SysResult {
        let end = start.checked_add(data.len()).ok_or(Errno::EFAULT)?;
        let mut copied = 0usize;
        let mut cur = start;

        while cur < end {
            let va = VirtAddr::from(cur);
            let vpn = va.floor();
            let page_offset = cur & (PAGE_SIZE - 1);
            let copy_len = (PAGE_SIZE - page_offset).min(end - cur);
            let pte = self.page_table.translate(vpn).ok_or(Errno::EFAULT)?;
            if !pte.is_valid() {
                return Err(Errno::EFAULT);
            }

            let dst = &mut pte.ppn().get_bytes_array()[page_offset..page_offset + copy_len];
            dst.copy_from_slice(&data[copied..copied + copy_len]);
            copied += copy_len;
            cur += copy_len;
        }
        Ok(())
    }

    // TOFIX: mmap 在分配内存时，如果分配跨过 2 MB 页表区间，则会导致第一个落进去的线程 TLS/启动栈会变成零
    // 目前对 mmap 起点的控制和处理属于为了通过测例的妥协之举
    #[cfg(target_arch = "loongarch64")]
    fn choose_mmap_start(&self, addr: Option<usize>, map_len: usize) -> SysResult<usize> {
        match addr {
            Some(start) => Ok(start),
            None => {
                let mut start = self.mmap_start;
                {
                    const PMD_SIZE: usize = PAGE_SIZE * 512;
                    let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
                    if map_len < PMD_SIZE && start / PMD_SIZE != (end - 1) / PMD_SIZE {
                        start = (start + PMD_SIZE - 1) & !(PMD_SIZE - 1);
                    }
                }
                Ok(start)
            }
        }
    }

    #[cfg(target_arch = "riscv64")]
    fn choose_mmap_start(&self, addr: Option<usize>, _map_len: usize) -> SysResult<usize> {
        match addr {
            Some(start) => Ok(start),
            None => Ok(self.mmap_start),
        }
    }

    /// 删除 mmap 区域，并在释放的是当前 mmap 尾部时回退下一次分配的起点。
    pub fn munmap_range(&mut self, addr: usize, map_len: usize) -> SysResult {
        let end = addr.checked_add(map_len).ok_or(Errno::EINVAL)?;
        if end > MMAP_MAX_ADDR {
            return Err(Errno::ENOMEM);
        }
        let vpn_range = VPNRange::new(VirtAddr::from(addr).floor(), VirtAddr::from(end).ceil());
        self.remove_area_with_overlap_range(vpn_range)?;
        if self.mmap_start == end {
            self.mmap_start = addr;
        }
        Ok(())
    }

    /// 惰性重映射：只修改 VPN 范围，不分配/释放物理页（由 page fault handler 按需处理）
    pub fn remap_area_lazy(
        &mut self,
        vpn_start: VirtPageNum,
        new_vpn_end: VirtPageNum,
    ) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| area.vpn_range.get_start() == vpn_start)
            .ok_or(Errno::EINVAL)?;

        if vpn_start == new_vpn_end {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
            return Ok(());
        }

        let old_vpn_end = area.vpn_range.get_end();
        if new_vpn_end < old_vpn_end {
            let vpn_range = VPNRange::new(new_vpn_end, old_vpn_end);
            for vpn in vpn_range {
                area.unmap_one(&mut self.page_table, vpn);
            }
        }
        area.vpn_range = VPNRange::new(vpn_start, new_vpn_end);
        Ok(())
    }

    /// 调整以指定页为结尾的可写用户区域，供 brk 在遇到前置保护页后继续维护堆尾。
    pub fn remap_writable_area_lazy_from_end(
        &mut self,
        old_vpn_end: VirtPageNum,
        new_vpn_end: VirtPageNum,
    ) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| {
                area.vpn_range.get_end() == old_vpn_end
                    && area
                        .map_perm
                        .contains(MapPermission::WRITE | MapPermission::USER)
            })
            .ok_or(Errno::EINVAL)?;

        let vpn_start = area.vpn_range.get_start();
        if vpn_start == new_vpn_end {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
            return Ok(());
        }

        if new_vpn_end < old_vpn_end {
            for vpn in VPNRange::new(new_vpn_end, old_vpn_end) {
                area.unmap_one(&mut self.page_table, vpn);
            }
        }
        area.vpn_range = VPNRange::new(vpn_start, new_vpn_end);
        Ok(())
    }

    /// 立即分配新增页的重映射版本，供 LoongArch 避开当前尚不稳定的用户堆 lazy fault 路径。
    pub fn remap_writable_area_eager_from_end(
        &mut self,
        old_vpn_end: VirtPageNum,
        new_vpn_end: VirtPageNum,
    ) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| {
                area.vpn_range.get_end() == old_vpn_end
                    && area
                        .map_perm
                        .contains(MapPermission::WRITE | MapPermission::USER)
            })
            .ok_or(Errno::EINVAL)?;

        let vpn_start = area.vpn_range.get_start();
        if vpn_start == new_vpn_end {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
            return Ok(());
        }

        if new_vpn_end < old_vpn_end {
            for vpn in VPNRange::new(new_vpn_end, old_vpn_end) {
                area.unmap_one(&mut self.page_table, vpn);
            }
        }
        area.vpn_range = VPNRange::new(vpn_start, new_vpn_end);
        if new_vpn_end > old_vpn_end {
            for vpn in VPNRange::new(old_vpn_end, new_vpn_end) {
                if !area.data_frames.contains_key(&vpn) {
                    area.map_one(&mut self.page_table, vpn)?;
                }
            }
        }
        Ok(())
    }

    /// 修改与给定虚拟页区间重叠的映射权限，必要时裁剪或切分原逻辑段
    pub fn remap_area_with_overlap_range(
        &mut self,
        vpn_range: VPNRange,
        map_perm: MapPermission,
    ) -> SysResult {
        let new_map_perm = map_perm | MapPermission::USER;

        for vpn in vpn_range {
            let area = self
                .areas
                .iter()
                .rev()
                .find(|area| area.vpn_range.contain(&vpn))
                .ok_or(Errno::ENOMEM)?;
            if !area.map_perm.contains(MapPermission::USER) {
                return Err(Errno::EFAULT);
            }
        }

        let mut idx = 0;
        while idx < self.areas.len() {
            if !self.areas[idx].vpn_range.intersect_with(&vpn_range) {
                idx += 1;
                continue;
            }

            let area_start = self.areas[idx].vpn_range.get_start();
            let area_end = self.areas[idx].vpn_range.get_end();
            let overlap_start = area_start.max(vpn_range.get_start());
            let overlap_end = area_end.min(vpn_range.get_end());

            // 重叠覆盖整个逻辑段——原地改权限即可，无需拆分
            if overlap_start == area_start && overlap_end == area_end {
                let area = &mut self.areas[idx];
                area.map_perm = new_map_perm;
                let mapped_vpns: Vec<_> = area.data_frames.keys().copied().collect();
                for vpn in mapped_vpns {
                    self.modify_user_pte_perm(vpn, new_map_perm);
                }
                idx += 1;
                continue;
            }

            // 需要拆分：取出当前 area，分成左/中/右三段
            let mut old_area = self.areas.remove(idx);
            let old_map_type = old_area.map_type;
            let old_map_perm = old_area.map_perm;

            // 拆分 data_frames
            let mut middle_and_right = old_area.data_frames.split_off(&overlap_start);
            let right_frames = middle_and_right.split_off(&overlap_end);

            // 修改中间段已映射页的 PTE 权限
            let mapped_vpns: Vec<_> = middle_and_right.keys().copied().collect();
            for vpn in mapped_vpns {
                self.modify_user_pte_perm(vpn, new_map_perm);
            }

            // 插入左段（旧权限）
            if overlap_start > area_start {
                self.areas.insert(
                    idx,
                    MapArea {
                        vpn_range: VPNRange::new(area_start, overlap_start),
                        data_frames: old_area.data_frames,
                        map_type: old_map_type,
                        map_perm: old_map_perm,
                    },
                );
                idx += 1;
            }

            // 插入中段（新权限）
            self.areas.insert(
                idx,
                MapArea {
                    vpn_range: VPNRange::new(overlap_start, overlap_end),
                    data_frames: middle_and_right,
                    map_type: old_map_type,
                    map_perm: new_map_perm,
                },
            );
            idx += 1;

            // 插入右段（旧权限）
            if overlap_end < area_end {
                self.areas.insert(
                    idx,
                    MapArea {
                        vpn_range: VPNRange::new(overlap_end, area_end),
                        data_frames: right_frames,
                        map_type: old_map_type,
                        map_perm: old_map_perm,
                    },
                );
                idx += 1;
            }
        }
        Ok(())
    }

    fn modify_user_pte_perm(&mut self, vpn: VirtPageNum, map_perm: MapPermission) {
        let Some(pte) = self.page_table.translate(vpn) else {
            return;
        };
        if !pte.is_valid() {
            return;
        }

        let mut flags = PTEFlags::from(map_perm);
        if pte.is_cow() {
            flags |= PTEFlags::COW;
            flags.remove(PTEFlags::WRITE);
        }
        self.page_table.modify_pte(vpn, flags);
    }

    /// 修改页表基址寄存器，切换页表
    pub fn flush_tlb(&self) {
        sfence();
    }

    /// 激活地址空间
    #[cfg(target_arch = "riscv64")]
    pub fn activate(&self) {
        let token = self.page_table.token();
        let vpn = VirtAddr::from(stext as *const () as usize).floor();
        let pte = self.page_table.translate(vpn).unwrap();
        assert!(pte.is_valid());
        assert!(pte.readable() || pte.executable() || pte.writable());

        write_mmu_token(token);
        self.flush_tlb();
    }

    #[cfg(target_arch = "loongarch64")]
    pub fn activate(&self) {
        let token = self.page_table.token();
        write_mmu_token(token);
        if !crate::arch::paging_enabled() {
            crate::arch::enable_mmu();
        }
        self.flush_tlb();
    }

    /// 生成页表对应 `stap` 寄存器值
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// 转译虚拟页号为物理页号
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }

    /// 遍历所有映射区域，供外部观察者（如 /proc/self/smaps）使用。
    ///
    /// 闭包参数依次为：起始虚拟地址、结束虚拟地址、权限。
    pub fn each_area(&self, mut f: impl FnMut(usize, usize, MapPermission)) {
        for area in self.areas.iter() {
            let start = area.vpn_range.get_start().0 << PAGE_SIZE_BITS;
            let end = area.vpn_range.get_end().0 << PAGE_SIZE_BITS;
            f(start, end, area.map_perm);
        }
    }

    /// 回收内部地址空间
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }
}

impl MemorySet {
    /// 创建一个新的地址空间，内部没有逻辑段
    pub fn new() -> Self {
        Self {
            brk: 0,
            heap_bottom: 0,
            mmap_start: MMAP_MIN_ADDR,
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    /// 创建一个拥有内核空间根页表页信息的地址空间，主要用于用户进程
    pub fn from_kernel_page_table() -> Self {
        Self {
            brk: 0,
            heap_bottom: 0,
            mmap_start: MMAP_MIN_ADDR,
            page_table: PageTable::from_kernel(),
            areas: Vec::new(),
        }
    }

    /// 创建内核地址空间
    ///
    /// 为内核地址建立虚拟地址，使其在虚拟地址开启时仍能正常访问内核空间，内核采用恒等映射
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new();
        // 尝试移除跳板映射
        // memory_set.map_trampoline();
        // 内核各段作为逻辑段加入地址空间
        // .text段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(stext as *const () as usize),
                VirtAddr::from(etext as *const () as usize),
                MapType::Direct,
                MapPermission::READ | MapPermission::EXECUTE,
            ),
            None,
            0,
        );
        // .rodata段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(srodata as *const () as usize),
                VirtAddr::from(erodata as *const () as usize),
                MapType::Direct,
                MapPermission::READ,
            ),
            None,
            0,
        );
        // .data段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(sdata as *const () as usize),
                VirtAddr::from(edata as *const () as usize),
                MapType::Direct,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None,
            0,
        );
        // .bss段和栈段（该栈段指的是初始分配的栈空间，初始化在.bss段）
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(sbss_with_stack as *const () as usize),
                VirtAddr::from(ebss as *const () as usize),
                MapType::Direct,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None,
            0,
        );
        // 内核剩余部分
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(ekernel as *const () as usize),
                VirtAddr::from(KERNEL_BASE + MEMORY_END),
                MapType::Direct,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None,
            0,
        );
        // 设备 MMIO 区域
        for (start, len) in VIRTIO_MMIO.iter().copied() {
            memory_set.push_empty_map_area(
                MapArea::new(
                    VirtAddr::from(KERNEL_BASE + start),
                    VirtAddr::from(KERNEL_BASE + start + len),
                    MapType::Direct,
                    MapPermission::READ | MapPermission::WRITE,
                ),
                None,
                0,
            );
        }

        memory_set
    }

    /// 根据 elf 格式的用户程序文件数据，创建用户程序内核空间
    ///
    /// 内部完成对elf文件的解析，当前内核对堆栈地址的处理能力不完善
    pub fn from_elf_data(elf_data: &[u8]) -> (Self, usize, usize, usize, Vec<AuxHeader>) {
        let mut memory_set = Self::from_kernel_page_table();

        // 在用户空间映射 sigreturn 跳板页
        memory_set.map_trampoline();
        // 由于传入的是 elf 格式的数据，所以需要读取文件头来得到各段的地址，之后再做分配映射
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap();
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let ph_entsize = elf_header.pt2.ph_entry_size() as usize;
        let mut entry_point = elf_header.pt2.entry_point() as usize;

        let mut max_vpn_end = VirtPageNum(0);
        let mut ph_va: usize = 0;
        let mut first_load: bool = true;
        let mut need_dl: bool = false;
        let mut interp_path: Option<alloc::string::String> = None;

        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            let ph_type = ph.get_type().unwrap();

            if ph_type == xmas_elf::program::Type::Load {
                let start_va = VirtAddr::from(ph.virtual_addr() as usize);
                let end_va = VirtAddr::from((ph.virtual_addr() + ph.mem_size()) as usize);
                if first_load {
                    // 第一个 LOAD 段的起始地址减去 ELF 头中的 ph_offset 即为程序头表虚拟地址
                    ph_va = start_va.0 + elf_header.pt2.ph_offset() as usize;
                    first_load = false;
                }

                let mut map_perm = MapPermission::USER;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::READ;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::WRITE;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::EXECUTE;
                }

                let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                max_vpn_end = map_area.vpn_range.get_end();
                let data_offset = start_va.0 & (PAGE_SIZE - 1);
                memory_set.push_empty_map_area(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                    data_offset,
                );
            }

            if ph_type == xmas_elf::program::Type::Interp {
                need_dl = true;
            }
        }

        // —— 读取 .interp section 获取动态链接器路径 ——
        if need_dl {
            if let Some(section) = elf.find_section_by_name(".interp") {
                let raw = section.raw_data(&elf);
                if let Ok(s) = core::str::from_utf8(raw) {
                    interp_path = Some(alloc::string::String::from(s.trim_end_matches('\0')));
                }
            }
        }

        // —— 加载动态链接器 ——
        if let Some(ref interp) = interp_path {
            // 尝试从内置应用数据中加载动态链接器
            if let Some(interp_data) = crate::loader::get_app_data_by_name(interp) {
                let interp_elf = xmas_elf::ElfFile::new(interp_data).unwrap();
                let interp_head = interp_elf.header;
                let interp_ph_count = interp_head.pt2.ph_count();
                entry_point = interp_head.pt2.entry_point() as usize + DL_INTERP_OFFSET;

                for i in 0..interp_ph_count {
                    let ph = interp_elf.program_header(i).unwrap();
                    if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                        let start_va =
                            VirtAddr::from(ph.virtual_addr() as usize + DL_INTERP_OFFSET);
                        let end_va = VirtAddr::from(
                            ph.virtual_addr() as usize + DL_INTERP_OFFSET + ph.mem_size() as usize,
                        );
                        let mut map_perm = MapPermission::USER;
                        let ph_flags = ph.flags();
                        if ph_flags.is_read() {
                            map_perm |= MapPermission::READ;
                        }
                        if ph_flags.is_write() {
                            map_perm |= MapPermission::WRITE;
                        }
                        if ph_flags.is_execute() {
                            map_perm |= MapPermission::EXECUTE;
                        }
                        let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                        let data_offset = start_va.0 & (PAGE_SIZE - 1);
                        memory_set.push_empty_map_area(
                            map_area,
                            Some(
                                &interp_data
                                    [ph.offset() as usize..(ph.offset() + ph.file_size()) as usize],
                            ),
                            data_offset,
                        );
                    }
                }

                info!(
                    "[from_elf_data] loaded dynamic linker: {} at {:#x}",
                    interp, DL_INTERP_OFFSET
                );
            } else {
                warn!(
                    "[from_elf_data] dynamic linker '{}' not found in embedded apps, proceeding without it",
                    interp
                );
                need_dl = false;
            }
        }

        // —— 构建 aux 向量 ——
        let mut aux_vec: Vec<AuxHeader> = alloc::vec![
            AuxHeader {
                aux_type: AT_PHDR,
                value: ph_va
            },
            AuxHeader {
                aux_type: AT_PHENT,
                value: ph_entsize
            },
            AuxHeader {
                aux_type: AT_PHNUM,
                value: ph_count as usize
            },
            AuxHeader {
                aux_type: AT_PAGESZ,
                value: PAGE_SIZE
            },
            AuxHeader {
                aux_type: AT_ENTRY,
                value: entry_point
            },
            AuxHeader {
                aux_type: AT_UID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_EUID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_GID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_EGID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_CLKTCK,
                value: CLK_TCK
            },
        ];

        if need_dl {
            aux_vec.push(AuxHeader {
                aux_type: AT_BASE,
                value: DL_INTERP_OFFSET,
            });
        }

        // 映射其余段
        let max_va_end = VirtAddr::from(max_vpn_end);
        let mut user_stack_bottom = usize::from(max_va_end);
        user_stack_bottom += PAGE_SIZE; // 上移栈底，将空白页作为守护页，当栈溢出时将访问守护页而发生段错误
        let user_stack_top = user_stack_bottom + USER_STACK_SIZE;
        // 映射栈段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(user_stack_bottom),
                VirtAddr::from(user_stack_top),
                MapType::Framed,
                MapPermission::READ | MapPermission::WRITE | MapPermission::USER,
            ),
            None,
            0,
        );
        // 映射堆段，初始不分配堆内存
        let heap_bottom = user_stack_top + PAGE_SIZE;
        memory_set.heap_bottom = heap_bottom;
        memory_set.brk = heap_bottom;

        // 不再映射异常上下文到用户空间，发生异常时，切换到内核将用户上下文存入内核栈

        let token = memory_set.page_table.token();
        (
            memory_set, // 用户程序地址空间
            token,
            user_stack_top, // 用户程序栈顶地址
            entry_point,    // 用户程序入口地址（动态链接时指向 ld-linux）
            aux_vec,        // auxiliary vector
        )
    }

    /// 基于已有用户地址空间创建新地址空间（用于 fork）
    ///
    /// 使用 COW（写时复制）策略：
    /// - 可写页：父子共享物理帧，两边 PTE 标记为只读 + COW
    /// - 只读页：直接共享物理帧，保持只读
    /// - 惰性未分配页：只复制 VPN 范围，各自按需分配
    pub fn from_existed_user(user_space: &mut MemorySet) -> Self {
        let mut memory_set = Self::from_kernel_page_table();
        memory_set.brk = user_space.brk;
        memory_set.heap_bottom = user_space.heap_bottom;
        memory_set.mmap_start = user_space.mmap_start;

        for area in user_space.areas.iter_mut() {
            let mut new_area = MapArea::from_another(area);
            let is_writable = area.map_perm.contains(MapPermission::WRITE);

            for vpn in area.vpn_range {
                if !area.data_frames.contains_key(&vpn) {
                    // 惰性未分配页：PTE 无效，data_frames 无记录
                    // 子进程也跳过，各自在访问时按需分配
                    continue;
                }

                let shared_frame = area.data_frames.get(&vpn).unwrap().clone();
                let ppn = shared_frame.ppn();

                if is_writable {
                    // COW 共享：标记父进程 PTE 为只读 + COW
                    let parent_pte = user_space.page_table.translate(vpn).unwrap();
                    let mut flags = parent_pte.flags();
                    flags.remove(PTEFlags::WRITE | PTEFlags::DIRTY);
                    user_space.page_table.modify_pte(vpn, flags);
                    user_space.page_table.make_pte_cow(vpn);

                    // 子进程 PTE 同样为只读 + COW
                    let mut child_flags = PTEFlags::from_bits(area.map_perm.bits).unwrap();
                    child_flags.remove(PTEFlags::WRITE | PTEFlags::DIRTY);
                    memory_set.page_table.map(vpn, ppn, child_flags);
                    memory_set.page_table.make_pte_cow(vpn);
                } else {
                    // 只读页直接共享，无需 COW
                    let child_flags = PTEFlags::from_bits(area.map_perm.bits).unwrap();
                    memory_set.page_table.map(vpn, ppn, child_flags);
                }
                new_area.data_frames.insert(vpn, shared_frame);
            }
            memory_set.areas.push(new_area);
        }
        user_space.flush_tlb();
        memory_set
    }
}

impl MemorySet {
    /// 检查用户传进来的虚拟地址的合法性
    ///
    /// 使用 `MapArea` 做检查，而不是查页表
    /// 要保证 `MapArea` 与页表的一致性，也就是说，页表中的映射都在MapArea中
    pub fn check_valid_user_vpn(
        &self,
        vpn: VirtPageNum,
        wanted_map_perm: MapPermission,
    ) -> SysResult<VPNRange> {
        let wanted = wanted_map_perm | MapPermission::USER;

        for area in self.areas.iter().rev() {
            if area.vpn_range.contain(&vpn) {
                if area.map_perm.contains(wanted) {
                    return Ok(area.vpn_range.clone());
                } else {
                    return Err(Errno::EFAULT);
                }
            }
        }
        Err(Errno::EFAULT)
    }
    pub fn check_valid_user_vpn_range(
        &self,
        vpn_range: VPNRange,
        wanted_map_perm: MapPermission,
    ) -> SysResult<VPNRange> {
        let wanted = wanted_map_perm | MapPermission::USER;

        for area in self.areas.iter().rev() {
            if area.vpn_range.contain_range(&vpn_range) {
                if area.map_perm.contains(wanted) {
                    return Ok(area.vpn_range.clone());
                } else {
                    return Err(Errno::EFAULT);
                }
            }
        }
        Err(Errno::EFAULT)
    }

    /// 检查一段用户虚拟页都属于用户映射区。
    ///
    /// 这里逐页检查，允许范围跨过相邻的多个用户 VMA。
    pub fn check_user_mapped_range(&self, vpn_range: VPNRange) -> SysResult {
        for vpn in vpn_range {
            self.check_valid_user_vpn(vpn, MapPermission::empty())?;
        }
        Ok(())
    }

    /// 尝试处理用户态页错误，解决 COW 或惰性分配
    pub fn handle_page_fault(&mut self, cause: PageFaultCause, stval: usize) -> SysResult {
        let vpn = VirtAddr::from(stval).floor();

        let area_idx = match self.areas.iter().position(|a| a.vpn_range.contain(&vpn)) {
            Some(idx) => idx,
            None => return Err(Errno::EFAULT),
        };

        let area_perm = self.areas[area_idx].map_perm;
        if !area_perm.contains(MapPermission::USER) {
            return Err(Errno::EFAULT);
        }

        let pte = self.page_table.translate(vpn);
        let is_store = matches!(cause, PageFaultCause::Store);

        // COW 写入：PTE 有效 + COW 标记 + area 允许写
        if is_store && pte.is_some_and(|p| p.is_valid() && p.is_cow()) {
            if !area_perm.contains(MapPermission::WRITE) {
                return Err(Errno::EFAULT);
            }

            let old_frame = self.areas[area_idx]
                .data_frames
                .get(&vpn)
                .ok_or(Errno::EFAULT)?;
            let count = Arc::strong_count(old_frame);

            if count == 1 {
                // 无其他进程共享，直接恢复可写
                let flags = PTEFlags::from(area_perm);
                self.page_table.modify_pte(vpn, flags);
                self.page_table.clear_pte_cow(vpn);
            } else {
                // 多进程共享时，换入一页新物理页并复制旧页内容
                let old_data = old_frame.ppn().get_bytes_array();
                self.areas[area_idx].remap_one_with_data(&mut self.page_table, vpn, old_data)?;
            }
            self.flush_tlb();
            return Ok(());
        }

        // 惰性分配：PTE 无效 + 在有效 area 内
        if pte.is_none() || !pte.unwrap().is_valid() {
            let needed_perm = if is_store {
                MapPermission::WRITE
            } else {
                match cause {
                    PageFaultCause::Instruction => MapPermission::EXECUTE,
                    _ => MapPermission::READ,
                }
            };

            if !area_perm.contains(needed_perm) {
                return Err(Errno::EFAULT);
            }

            self.areas[area_idx].map_one(&mut self.page_table, vpn)?;
            self.flush_tlb();
            return Ok(());
        }

        Err(Errno::EFAULT)
    }

    pub fn ensure_user_page_access(
        &mut self,
        vpn_range: VPNRange,
        perm: MapPermission,
    ) -> SysResult {
        for vpn in vpn_range {
            let pte = self.page_table.translate(vpn);
            let needs_fault = match pte {
                Some(pte) if pte.is_valid() => {
                    perm.contains(MapPermission::WRITE) && (!pte.writable() || pte.is_cow())
                }
                _ => true,
            };
            if !needs_fault {
                continue;
            }

            let cause = if perm.contains(MapPermission::WRITE) {
                PageFaultCause::Store
            } else if perm.contains(MapPermission::EXECUTE) {
                PageFaultCause::Instruction
            } else {
                PageFaultCause::Load
            };
            let va = usize::from(VirtAddr::from(vpn));
            self.handle_page_fault(cause, va)?;
        }
        Ok(())
    }
}

/// 逻辑段
///
/// 一段连续地址 [`VPNRange`] 的虚拟内存
struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
    map_type: MapType,
    map_perm: MapPermission,
}

impl MapArea {
    /// 创建空逻辑段
    ///
    /// 只指定了一段虚拟内存，内部没有实际的映射的页帧
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
    ) -> Self {
        let start_vpn: VirtPageNum = start_va.floor();
        let end_vpn: VirtPageNum = end_va.ceil();
        Self {
            vpn_range: VPNRange::new(start_vpn, end_vpn),
            data_frames: BTreeMap::new(),
            map_type,
            map_perm,
        }
    }
    /// 复制构造空逻辑段
    ///
    /// 只指定了一段与传入逻辑段一致的虚拟内存，内部没有实际的映射的页帧
    pub fn from_another(another: &MapArea) -> Self {
        Self {
            vpn_range: VPNRange::new(another.vpn_range.get_start(), another.vpn_range.get_end()),
            data_frames: BTreeMap::new(),
            map_type: another.map_type,
            map_perm: another.map_perm,
        }
    }

    /// 为逻辑段上所有虚拟页创建物理页帧并建立映射
    ///
    /// 传入页表的可变借用，以修改传入页表的内容
    pub fn map(&mut self, page_table: &mut PageTable) -> SysResult {
        for vpn in self.vpn_range {
            self.map_one(page_table, vpn)?;
        }
        Ok(())
    }
    /// 为逻辑段上所有虚拟页销毁物理页帧并消除映射
    pub fn unmap(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }

    /// 复制数据到逻辑段的实际物理页帧上
    pub fn copy_data(&mut self, page_table: &PageTable, data: &[u8], offset: usize) {
        assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        // 数据长度不超过逻辑段长度
        assert!(
            len <= PAGE_SIZE * (self.vpn_range.get_end() - current_vpn),
            "[kernel] MapArea Panic: Copy data is out of vpn range!"
        );
        // 数据在段内有偏移，对第一页做特殊处理
        if offset != 0 {
            let src = &data[0..len.min(PAGE_SIZE - offset)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[offset..offset + src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE - offset;
            current_vpn.step();
        }
        loop {
            if start >= len {
                break;
            }
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            current_vpn.step();
        }
    }

    /// 依据逻辑段的不同映射策略，为虚拟页分配物理页帧，并建立映射关系
    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) -> SysResult {
        let ppn: PhysPageNum;
        match self.map_type {
            // 直接映射，物理页号和虚拟页号存在线性偏移，一般用于内核，无需分配页帧管理，因为内存地址固定
            MapType::Direct => {
                ppn = PhysPageNum(vpn - VirtAddr::from(KERNEL_BASE).floor());
            }
            // 随机映射，物理页号和虚拟页号无关，用于用户程序，分配页帧统一管理
            MapType::Framed => {
                let frame = frame_alloc().ok_or(Errno::ENOMEM)?;
                ppn = frame.ppn();
                self.data_frames.insert(vpn, Arc::new(frame));
            }
        }
        let pte_flags = PTEFlags::from(self.map_perm);
        page_table.map(vpn, ppn, pte_flags);
        Ok(())
    }

    /// 为已有映射换入一页新的物理页，并把给定数据复制进去
    pub fn remap_one_with_data(
        &mut self,
        page_table: &mut PageTable,
        vpn: VirtPageNum,
        data: &[u8],
    ) -> SysResult {
        let frame = frame_alloc().ok_or(Errno::ENOMEM)?;
        let ppn = frame.ppn();
        // 由调用者保证 data 合法
        ppn.get_bytes_array().copy_from_slice(data);

        page_table.unmap(vpn);
        self.data_frames.insert(vpn, Arc::new(frame));
        page_table.map(vpn, ppn, PTEFlags::from(self.map_perm));
        Ok(())
    }

    /// 消除虚拟页与物理页帧的映射关系，自动销毁失去连接的物理页帧
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.map_type {
            MapType::Framed => {
                self.data_frames.remove(&vpn);
            }
            _ => {}
        }
        page_table.try_unmap(vpn);
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum MapType {
    Direct, // 直接映射——线性偏移
    Framed, // 随机映射
}

bitflags! {
    pub struct MapPermission: u16 {
        const READ     = 1 << 1;
        const WRITE    = 1 << 2;
        const EXECUTE  = 1 << 3;
        const USER     = 1 << 4;
        const GLOBAL   = 1 << 5;
        const ACCESSED = 1 << 6;
        const DIRTY    = 1 << 7;
    }
}
