// os/src/mm/memory_set.rs

use riscv::register::satp;
use lazy_static::lazy_static;
use bitflags::bitflags;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::sync::Arc;
use core::arch::asm;
use crate::config::{ PAGE_SIZE, KERNEL_MEM_END, USER_STACK_SIZE, TRAMPOLINE, TRAP_CONTEXT };
use crate::sync::UPSafeCell;
use super::address::{ PhysAddr, PhysPageNum, VirtAddr, VirtPageNum, VPNRange, StepByOne };
use super::frame_allocator::{ frame_alloc, FrameTracker };
use super::page_table::{ PageTable, PTEFlags, PageTableEntry };

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
    fn strampoline();
}


lazy_static! {
    /// 内核地址空间，内核地址空间在内核初始化后创建
    /// 
    /// 内核采用恒等映射，因而开启虚拟地址后访问内核空间的地址不变
    /// 
    /// 由于内核空间被所有用户空间共享，所以使用 [`Arc`] 来实现共享，使用 [`UPSafeCell`] 来实现内部可变性
    pub static ref KERNEL_SPACE: Arc<UPSafeCell<MemorySet>> = Arc::new(unsafe {
        UPSafeCell::new(MemorySet::new_kernel())
    });
}


/// 地址空间
/// 
/// 一系列有关联的逻辑段 [`MapArea`]，地址不一定连续
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}

impl MemorySet {
    /// 创建一个新的地址空间，内部没有逻辑段
    pub fn new() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    /// 对外暴露的添加逻辑段的接口，只支持添加内核栈段
    pub fn insert_stack_area(&mut self, start_va: VirtAddr, end_va: VirtAddr) {
        self.push_empty_map_area (
            MapArea::new(
                start_va,
                end_va,
                MapType::Framed,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None
        )
    }
    /// 将一段空的逻辑段加入地址空间，在内部完成映射关系的建立
    fn push_empty_map_area(&mut self, mut map_area: MapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data);
        }
        self.areas.push(map_area); // 转移所有权
    }

    /// 激活地址空间，即修改 `stap` ，切换页表
    pub fn activate(&self) {
        let stap = self.page_table.token();
        let vpn = VirtAddr::from(stext as *const () as usize).floor();
        let pte = self.page_table.translate(vpn).unwrap();
        assert!(pte.is_valid());
        assert!(pte.readable() || pte.executable() || pte.writable());

        unsafe {
            satp::write(stap);
            asm!("sfence.vma"); // 屏障，可认为是刷新 TLB，确保页表修改生效
        }
    }

    /// 生成页表对应 `stap` 寄存器值
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// 转译虚拟页号为物理页号
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }
}

impl MemorySet {
    /// 映射跳板空间
    /// 
    /// 跳板是一个特殊的区域，位于虚拟地址空间的最高端，大小为一页
    /// 跳板提供一段用户和内核空间都能访问的内存区域
    /// 在用户程序发生异常时，内核能够通过跳板访问用户程序的上下文信息
    /// 
    /// 鉴于其特殊性，跳板不作为一个逻辑段加入地址空间，而是单独映射
    pub fn map_trampoline(&mut self) {
        self.page_table.map(
            VirtAddr::from(TRAMPOLINE).into(),
            PhysAddr::from(strampoline as *const() as usize).into(),
            PTEFlags::READ | PTEFlags::EXECUTE,
        );
    }

    /// 创建内核地址空间
    /// 
    /// 为内核地址建立虚拟地址，使其在虚拟地址开启时仍能正常访问内核空间，内核采用恒等映射
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new();
        // 映射跳板
        memory_set.map_trampoline();
        // 内核各段作为逻辑段加入地址空间
        // .text段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(stext as *const () as usize),
                VirtAddr::from(etext as *const () as usize),
                MapType::Identical,
                MapPermission::READ | MapPermission::EXECUTE
            ),
            None
        );
        // .rodata段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(srodata as *const () as usize),
                VirtAddr::from(erodata as *const () as usize),
                MapType::Identical,
                MapPermission::READ
            ),
            None
        );
        // .data段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(sdata as *const () as usize),
                VirtAddr::from(edata as *const () as usize),
                MapType::Identical,
                MapPermission::READ | MapPermission::WRITE
            ),
            None
        );
        // .bss段和栈段（该栈段指的是初始分配的栈空间，初始化在.bss段）
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(sbss_with_stack as *const () as usize),
                VirtAddr::from(ebss as *const () as usize),
                MapType::Identical,
                MapPermission::READ | MapPermission::WRITE
            ),
            None
        );
        // 内核剩余部分
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(ekernel as *const () as usize),
                VirtAddr::from(KERNEL_MEM_END),
                MapType::Identical,
                MapPermission::READ | MapPermission::WRITE
            ),
            None
        );
        memory_set
    }

    /// 根据 elf 格式的用户程序文件数据，创建用户程序内核空间
    /// 
    /// 内部完成对elf文件的解析，当前内核对堆栈地址的处理能力不完善
    /// 
    /// TODO: 返回值为三元组，为规范之后将改用结构体返回
    pub fn from_elf_data(elf_data: &[u8]) -> (Self, usize, usize) {
        let mut memory_set = Self::new();
        // 映射跳板
        memory_set.map_trampoline();
        // 由于传入的是 elf 格式的数据，所以需要读取文件头来得到各段的地址，之后再做分配映射
        // TODO: 此处对于 elf 格式的解析仍依赖于外部库，鉴于读取头文件信息的功能相对简单，建议考虑之后自己实现
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
                if ph_flags.is_read() { map_perm |= MapPermission::READ; }
                if ph_flags.is_write() { map_perm |= MapPermission::WRITE; }
                if ph_flags.is_execute() { map_perm |= MapPermission::EXECUTE; }

                let map_area = MapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    map_perm,
                );
                max_vpn_end = map_area.vpn_range.get_end();
                memory_set.push_empty_map_area(
                    map_area, 
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize])
                );
            }
        }

        // 映射其余段
        let max_va_end= VirtAddr::from(max_vpn_end);
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
        );
        // 映射堆段，当前没有堆段，暂时不映射
        // 映射异常上下文，位于次高地址
        // 注：在创建用户任务时，地址空间中已经完成了异常上下文分配，要修改内部数据可通过固定虚拟地址实现
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(TRAP_CONTEXT),
                VirtAddr::from(TRAMPOLINE),
                MapType::Framed,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None
        );

        (
            memory_set, // 用户程序地址空间
            user_stack_top, // 用户程序栈顶地址
            elf.header.pt2.entry_point() as usize // 用户程序入口地址
        )
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
        map_perm: MapPermission
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

    /// 为逻辑段上所有虚拟页创建物理页帧并建立映射
    /// 
    /// 传入页表的可变引用，以修改传入页表的内容
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
    pub fn copy_data(&mut self, _page_table: &PageTable, data: &[u8]) {
        assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        // 数据长度不超过逻辑段长度
        assert!(
            len <= PAGE_SIZE * (self.vpn_range.get_end().0 - current_vpn.0),
            "[kernel] MapArea Panic: Copy data is out of vpn range!"
        );
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut self.data_frames
                .get_mut(&current_vpn) // FIXME: 由于 BTreeMap 查询效率较低，可能会被优化掉
                .unwrap()
                .bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len { break; }
            current_vpn.step();
        }
    }

    /// 依据逻辑段的不同映射策略，为虚拟页分配物理页帧，并建立映射关系
    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        match self.map_type {
            // 恒等映射，物理页号和虚拟页号一致，一般用于内核，无需分配页帧管理，因为内存地址固定
            MapType::Identical => { ppn = PhysPageNum(vpn.0); }
            // 随机映射？物理页号和虚拟页号无关，用于用户程序，分配页帧统一管理
            MapType::Framed => {
                let frame = frame_alloc().unwrap();
                ppn = frame.ppn();
                self.data_frames.insert(vpn, frame);
            }
        }
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }

    #[allow(unused)]
    /// 消除虚拟页与物理页帧的映射关系，自动销毁失去连接的物理页帧
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.map_type {
            MapType::Framed => { self.data_frames.remove(&vpn); }
            _ => {}
        }
        page_table.unmap(vpn);
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum MapType {
    Identical, // 恒等映射
    Framed,    // 随机映射
}

bitflags! {
    pub struct MapPermission: u8 {
        const READ =    1 << 1;
        const WRITE =   1 << 2;
        const EXECUTE = 1 << 3;
        const USER =    1 << 4;
    }
}
