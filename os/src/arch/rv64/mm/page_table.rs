// os/src/arch/rv64/mm/page_table.rs

use crate::config::KERNEL_BASE;
use crate::mm::{
    FrameTracker, KERNEL_SPACE, MapPermission, PPN_WIDTH_SV39, PhysAddr, PhysPageNum, VirtAddr,
    VirtPageNum, frame_alloc,
};
use alloc::string::String;
use alloc::{vec, vec::Vec};
use bitflags::*;

/// 页表
///
/// - [`PhysPageNum`] 根页表页帧的物理页号
/// - [`Vec<FrameTracker>`] 页表占用的物理页帧的集合
pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>, // 追踪页表占用的物理页帧，自动回收
}

impl PageTable {
    /// 创建页表——仅创建根页表
    pub fn new() -> Self {
        let frame = frame_alloc().expect("Failed to allocate frame for page table");
        Self {
            root_ppn: frame.ppn(),
            frames: vec![frame],
        }
    }
    /// 依据内核空间页表创建新页表
    pub fn from_kernel() -> Self {
        let frame = frame_alloc().unwrap();
        let kernel_page_table = &KERNEL_SPACE.lock().page_table;
        let kernel_root_ppn = kernel_page_table.root_ppn;
        // 拷贝内核空间的根页表页
        let index = VirtAddr::from(KERNEL_BASE).floor().indexes()[0];
        frame.ppn().get_pte_array()[index..]
            .copy_from_slice(&kernel_root_ppn.get_pte_array()[index..]);
        PageTable {
            root_ppn: frame.ppn(),
            frames: vec![frame],
        }
    }
    /// 临时页表无数据，仅用于查询用户程序的数据
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from(satp & ((1usize << 44) - 1)),
            frames: Vec::new(),
        }
    }

    /// 生成页表对应 `stap` 寄存器值
    pub fn token(&self) -> usize {
        (8usize << 60) | self.root_ppn.0
    }
    /// 转译虚拟页号为对应页表项
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }
    /// 转译虚拟地址为物理地址
    pub fn translate_va(&self, va: VirtAddr) -> Option<PhysAddr> {
        self.find_pte(va.clone().floor()).map(|pte| {
            let aligned_pa: PhysAddr = pte.ppn().into();
            let offset = va.page_offset();
            let aligned_pa_usize: usize = aligned_pa.into();
            (aligned_pa_usize + offset).into()
        })
    }
}

pub fn translated_ref<T>(token: usize, ptr: *const T) -> &'static T {
    let page_table = PageTable::from_token(token);
    page_table
        .translate_va(VirtAddr::from(ptr as usize))
        .unwrap()
        .get_ref()
}

pub fn translated_refmut<T>(token: usize, ptr: *mut T) -> &'static mut T {
    let page_table = PageTable::from_token(token);
    let va = ptr as usize;
    page_table
        .translate_va(VirtAddr::from(va))
        .unwrap()
        .get_mut()
}

/// 页表设置页表项接口
///
/// 均返回页表项的可变借用，用于修改或读取页表项
impl PageTable {
    /// 根据虚拟地址寻找目标页表项，若发现多级页表不存在则创建
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for i in 0..3 {
            let pte = &mut ppn.get_pte_array()[idxs[i]];
            if i == 2 {
                // 直接返回目标的页表项，
                result = Some(pte);
                break;
            }
            // 页表页由页帧分配器分配得到，默认为空，没有修改的页表项均无效
            if !pte.is_valid() {
                // 当前页表项无效，表示当前非叶子页表页的孩子页表页不存在，创建新的页表页
                let frame = frame_alloc().unwrap();
                // 更新当前页表页上的页表项
                *pte = PageTableEntry::new(frame.ppn(), PTEFlags::VALID);
                // 加入页表，由页表统一维护页表页帧
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        result
    }

    /// 根据虚拟地址寻找目标页表项
    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for i in 0..3 {
            let pte = &mut ppn.get_pte_array()[idxs[i]];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                break;
            }
            ppn = pte.ppn();
        }
        result
    }
}

impl PageTable {
    /// 在页表中建立物理地址和虚拟地址的映射关系
    ///
    /// 一般用于初始化一个新分配的物理页帧，所以还需要页表项标志位数据
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();
        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn); // 页表项应当没有被创建
        *pte = PageTableEntry::new(
            ppn,
            flags | PTEFlags::VALID | PTEFlags::ACCESSED | PTEFlags::DIRTY,
        ); // 被映射的物理页帧必定有效，这里需要统一配置
    }
    /// 在页表中消除物理页和虚拟页的映射关系
    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
    }
    /// 尝试在页表中消除物理页和虚拟页的映射关系，若该虚拟页未映射物理页则直接返回
    pub fn try_unmap(&mut self, vpn: VirtPageNum) {
        let pte = if let Some(page_table_entry) = self.find_pte(vpn) {
            page_table_entry
        } else {
            return;
        };
        if !pte.is_valid() {
            return;
        }
        *pte = PageTableEntry::empty();
    }

    /// 修改已有映射的标志位（不改变物理页号），用于 COW 恢复等场景
    pub fn modify_pte(&mut self, vpn: VirtPageNum, flags: PTEFlags) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid in modify_pte", vpn);
        *pte = PageTableEntry::new(
            pte.ppn(),
            flags | PTEFlags::VALID | PTEFlags::ACCESSED | PTEFlags::DIRTY,
        );
    }
    /// 设置页表项的 COW 标记位
    pub fn set_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        pte.set_cow_bit();
    }
    pub fn make_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        pte.set_cow_bit();
    }
    /// 清除页表项的 COW 标记位
    pub fn clear_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        pte.clear_cow_bit();
    }
}

/// 页表项
///
/// | Reserved | PPN   | RSW | COW | D | A | G | U | X | W | R | V |
/// | -------- | ----- | --- | --- | - | - | - | - | - | - | - | - |
/// | 63-54    | 53-10 |  9  |  8  | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize, // 包含物理页号和标志位
}

// 页表项的标志位
bitflags! {
    pub struct PTEFlags: u16 {
        const VALID    = 1 << 0;
        const READ     = 1 << 1;
        const WRITE    = 1 << 2;
        const EXECUTE  = 1 << 3;
        const USER     = 1 << 4;
        const GLOBAL   = 1 << 5;
        const ACCESSED = 1 << 6;
        const DIRTY    = 1 << 7;
        const COW      = 1 << 8;
    }
}

#[allow(unused)]
impl PTEFlags {
    pub fn readable_flags(&self) -> String {
        let mut ret = String::new();
        if self.contains(PTEFlags::VALID) {
            ret.push_str("V");
        }
        if self.contains(PTEFlags::READ) {
            ret.push_str("R");
        }
        if self.contains(PTEFlags::WRITE) {
            ret.push_str("W");
        }
        if self.contains(PTEFlags::EXECUTE) {
            ret.push_str("X");
        }
        if self.contains(PTEFlags::USER) {
            ret.push_str("U");
        }
        if self.contains(PTEFlags::GLOBAL) {
            ret.push_str("G");
        }
        if self.contains(PTEFlags::ACCESSED) {
            ret.push_str("A");
        }
        if self.contains(PTEFlags::DIRTY) {
            ret.push_str("D");
        }
        ret
    }
}

impl From<MapPermission> for PTEFlags {
    fn from(value: MapPermission) -> Self {
        PTEFlags::from_bits(value.bits()).unwrap()
    }
}

impl PageTableEntry {
    /// 创建一个新的页表项
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        Self {
            bits: (ppn.0 << 10) | (flags.bits as usize),
        }
    }
    /// 创建一个空的（无效的）页表项
    pub fn empty() -> Self {
        Self { bits: 0 }
    }

    /// 获取物理页号
    pub fn ppn(&self) -> PhysPageNum {
        PhysPageNum((self.bits >> 10) & ((1usize << PPN_WIDTH_SV39) - 1))
    }
    /// 获取标志位
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.bits as u16)
    }

    /// 判断页表项是否有效
    pub fn is_valid(&self) -> bool {
        self.flags().contains(PTEFlags::VALID)
    }
    /// 判断指定页是否可读
    pub fn readable(&self) -> bool {
        self.flags().contains(PTEFlags::READ)
    }
    /// 判断指定页是否可写
    pub fn writable(&self) -> bool {
        self.flags().contains(PTEFlags::WRITE)
    }
    /// 判断指定页是否可执行
    pub fn executable(&self) -> bool {
        self.flags().contains(PTEFlags::EXECUTE)
    }
    /// 判断是否是写时复制页面
    pub fn is_cow(&self) -> bool {
        self.flags().contains(PTEFlags::COW)
    }
}

impl PageTableEntry {
    /// 设置 COW 标记位
    pub fn set_cow_bit(&mut self) {
        self.bits |= 1 << 8;
    }
    /// 清除 COW 标记位
    pub fn clear_cow_bit(&mut self) {
        self.bits &= !(1 << 8);
    }
}

// 重构地址空间后内核可以直接访问用户空间中的数据，无需再额外转译

// /// 转译虚拟地址，得到内核虚拟地址上的一段数据的可变借用
// ///
// /// 以向量返回，其内部的每个数据代表单个内存页上的数据的可变借用
// pub fn translate_byte_buffer(token: usize, ptr: *const u8, len: usize) -> Vec<&'static mut [u8]> {
//     let page_table = PageTable::from_token(token);
//     let mut start = ptr as usize;
//     let end = start + len;
//     let mut v = Vec::new();
//     while start < end {
//         let start_va = VirtAddr::from(start);
//         let mut vpn = start_va.floor();
//         let ppn = page_table.translate(vpn).unwrap().ppn();
//         vpn.step();
//         let mut end_va = VirtAddr::from(vpn);
//         end_va = end_va.min(VirtAddr::from(end));
//         if end_va.page_offset() == 0 {
//             v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..]);
//         } else {
//             v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..end_va.page_offset()])
//         }
//         start = end_va.into();
//     }
//     v
// }

// /// 转译虚拟地址，得到内核虚拟地址上以此为始的一个字符串
// pub fn translate_str(token: usize, ptr: *const u8) -> String {
//     let page_table = PageTable::from_token(token);
//     let mut string = String::new();
//     let mut va = ptr as usize;
//     loop {
//         let ch: u8 = *page_table.translate_va(VirtAddr::from(va)).unwrap().get_mut();
//         if ch == 0 { break; }
//         else {
//             string.push(ch as char);
//             va += 1;
//         }
//     }
//     string
// }

// /// 转译虚拟地址，得到内核虚拟地址上对应数据的可变引用
// pub fn translated_refmut<T>(token: usize, ptr: *mut T) -> &'static mut T {
//     let page_table = PageTable::from_token(token);
//     let va = ptr as usize;
//     page_table.translate_va(VirtAddr::from(va)).unwrap().get_mut()
// }
