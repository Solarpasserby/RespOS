// os/src/mm/page_table.rs

use bitflags::*;
use alloc::{ vec, vec::Vec };
use crate::config::PAGE_SIZE;
use super::address::{ PPN_WIDTH_SV39, PhysAddr, PhysPageNum, VirtPageNum };
use super::frame_allocator::{ FrameTracker, frame_alloc };

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

    /// 生成页表对应 `stap` 寄存器值
    pub fn token(&self) -> usize {
        8usize << 60 | self.root_ppn.0
    }
    /// 转译虚拟页号为物理页号
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }
}

/// 页表设置页表项接口
/// 
/// 均返回页表项的可变引用，用于修改或读取页表项
impl PageTable {
    /// 根据虚拟地址寻找目标页表项，若发现多级页表不存在则创建
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for i in 0..3 {
            let pte = &mut get_pte_array(&ppn)[idxs[i]];
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
            let pte = &mut get_pte_array(&ppn)[idxs[i]];
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
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::VALID); // 被映射的物理页帧必定有效，这里需要统一配置
    }
    /// 在页表中消除物理地址和虚拟地址的映射关系
    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
    }
}


/// 页表项
/// 
/// | Reserved | PPN   | RSW | D | A | G | U | X | W | R | V |
/// | -------- | ----- | --- | - | - | - | - | - | - | - | - |
/// | 63-54    | 53-10 | 9-8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize, // 包含物理页号和标志位
}

// 页表项的标志位
bitflags! {
    pub struct PTEFlags: u8 {
        const VALID    = 1 << 0;
        const READ     = 1 << 1;
        const WRITE    = 1 << 2;
        const EXECUTE  = 1 << 3;
        const USER     = 1 << 4;
        const GLOBAL   = 1 << 5;
        const ACCESSED = 1 << 6;
        const DIRTY    = 1 << 7;
    }
}

impl PageTableEntry {
    /// 创建一个新的页表项
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        Self { bits: ppn.0 << 10 | (flags.bits as usize) }
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
        PTEFlags::from_bits(self.bits as u8).unwrap()
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
}


/// 获取页表项数组（依赖物理页表）
/// 
/// 由于页表项是一段 `usize` 的数据，因此实际上只是返回一段可变数组切片
/// 
/// 由于需要递归查询页表，不得不实现该功能
/// 现在又将其从 [`PhysPageNum`] 的方法中转移出来限制其使用
fn get_pte_array(ppn: &PhysPageNum) -> &'static mut [PageTableEntry] {
    let pa = PhysAddr::from(*ppn);
    unsafe { core::slice::from_raw_parts_mut(pa.0 as *mut PageTableEntry, PAGE_SIZE / core::mem::size_of::<PageTableEntry>()) }
}
