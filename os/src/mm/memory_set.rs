// os/src/mm/memory_set.rs

use super::address::{PhysPageNum, StepByOne, VPNRange, VirtAddr, VirtPageNum};
use super::frame_allocator::{FrameTracker, frame_alloc};
use super::{PTEFlags, PageTable, PageTableEntry};
use crate::arch::{sfence, write_mmu_token};
use crate::config::{
    KERNEL_BASE, KERNEL_STACK_SIZE, MEMORY_END, MMAP_MIN_ADDR, MMIO, PAGE_SIZE, USER_STACK_SIZE,
};
use crate::syscall::{Errno, SysResult};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use lazy_static::lazy_static;
use spin::Mutex;

/// 用户态 sigreturn 跳板页的虚拟地址。
/// 该页在所有用户进程的地址空间中映射到同一位置。
pub const TRAMPOLINE: usize = 0x0000_003f_ffff_f000;

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
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data, data_offset);
        }
        self.areas.push(map_area); // 转移所有权
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
    /// 用户态信号处理函数返回后，会跳转到该页执行 `li a7,139; ecall`，
    /// 从而进入内核的 sys_sigreturn。每个用户进程都需要此映射。
    pub fn map_trampoline(&mut self) {
        // `li a7, 139; ecall` 的 RISC-V 机器码。
        // 这段跳板代码用于从用户态信号处理函数返回后，发起 sigreturn 系统调用。
        const TRAMPOLINE_CODE: [u8; 8] = [
            0x93, 0x08, 0xb0, 0x08, // addi x17, x0, 139  (li a7, 139)
            0x73, 0x00, 0x00, 0x00, // ecall
        ];
        // 在用户空间分配一页并映射
        self.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(TRAMPOLINE),
                VirtAddr::from(TRAMPOLINE + PAGE_SIZE),
                MapType::Framed,
                MapPermission::READ | MapPermission::EXECUTE | MapPermission::USER,
            ),
            None,
            0,
        );
        // 把跳板指令复制到用户跳板页
        let trampoline_page = self
            .page_table
            .translate(VirtAddr::from(TRAMPOLINE).floor())
            .unwrap()
            .ppn();
        let dst = &mut trampoline_page.get_bytes_array();
        dst[..8].copy_from_slice(&TRAMPOLINE_CODE);
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

    /// 根据首虚拟页号重新映射对应逻辑段（逻辑段的大小发生变化）
    pub fn remap_area_with_start_vpn(
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
        let page_table = &mut self.page_table;
        if new_vpn_end > old_vpn_end {
            let vpn_range = VPNRange::new(old_vpn_end, new_vpn_end);
            for vpn in vpn_range {
                area.map_one(page_table, vpn);
            }
        } else {
            let vpn_range = VPNRange::new(new_vpn_end, old_vpn_end);
            for vpn in vpn_range {
                area.unmap_one(page_table, vpn);
            }
        }
        area.vpn_range = VPNRange::new(vpn_start, new_vpn_end);
        Ok(())
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
        self.flush_tlb(); // 内存屏障，刷新 TLB，确保之后内存读写正确
    }

    /// 生成页表对应 `stap` 寄存器值
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// 转译虚拟页号为物理页号
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
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
        // MMIO 部分
        for (start, len) in MMIO {
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
    pub fn from_elf_data(elf_data: &[u8]) -> (Self, usize, usize, usize) {
        let mut memory_set = Self::from_kernel_page_table();

        // 在用户空间映射 sigreturn 跳板页
        memory_set.map_trampoline();
        // 由于传入的是 elf 格式的数据，所以需要读取文件头来得到各段的地址，之后再做分配映射
        // 也正是因为是外部库，我对这部分的细节不是非常了解
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap();
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_vpn_end = VirtPageNum(0);
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let start_va = VirtAddr::from(ph.virtual_addr() as usize);
                let end_va = VirtAddr::from((ph.virtual_addr() + ph.mem_size()) as usize);
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
            user_stack_top,                        // 用户程序栈顶地址
            elf.header.pt2.entry_point() as usize, // 用户程序入口地址
        )
    }

    pub fn from_existed_user(user_space: &MemorySet) -> Self {
        let mut memory_set = Self::from_kernel_page_table();
        // 复制 heap_bottom 和 brk
        memory_set.brk = user_space.brk;
        memory_set.heap_bottom = user_space.heap_bottom;

        // 映射并复制各段，堆的内容也被复制
        for area in user_space.areas.iter() {
            let new_area = MapArea::from_another(area);
            memory_set.push_empty_map_area(new_area, None, 0);
            for vpn in area.vpn_range {
                // 两个逻辑段的虚拟地址一致
                let src = user_space.translate(vpn).unwrap().ppn().get_bytes_array();
                let dst = memory_set.translate(vpn).unwrap().ppn().get_bytes_array();
                dst.copy_from_slice(src);
            }
        }
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
}

/// 逻辑段
///
/// 一段连续地址 [`VPNRange`] 的虚拟内存
struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
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
    pub fn map(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.map_one(page_table, vpn);
        }
    }
    /// 为逻辑段上所有虚拟页销毁物理页帧并消除映射
    #[allow(unused)]
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
    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        match self.map_type {
            // 直接映射，物理页号和虚拟页号存在线性偏移，一般用于内核，无需分配页帧管理，因为内存地址固定
            MapType::Direct => {
                ppn = PhysPageNum(vpn - VirtAddr::from(KERNEL_BASE).floor());
            }
            // 随机映射，物理页号和虚拟页号无关，用于用户程序，分配页帧统一管理
            MapType::Framed => {
                let frame = frame_alloc().unwrap();
                ppn = frame.ppn();
                self.data_frames.insert(vpn, frame);
            }
        }
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }

    /// 消除虚拟页与物理页帧的映射关系，自动销毁失去连接的物理页帧
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.map_type {
            MapType::Framed => {
                self.data_frames.remove(&vpn);
            }
            _ => {}
        }
        page_table.unmap(vpn);
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum MapType {
    Direct, // 直接映射——线性偏移
    Framed, // 随机映射
}

bitflags! {
    pub struct MapPermission: u8 {
        const READ     = 1 << 1;
        const WRITE    = 1 << 2;
        const EXECUTE  = 1 << 3;
        const USER     = 1 << 4;
        const GLOBAL   = 1 << 5;
        const ACCESSED = 1 << 6;
        const DIRTY    = 1 << 7;
    }
}
