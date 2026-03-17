// os/src/mm/page_table.rs

use bitflags::*;
use alloc::{ vec, vec::Vec };
use super::address::{ PPN_WIDTH_SV39, PhysPageNum, VirtPageNum };
use super::frame_allocator::{ FrameTracker, frame_alloc };


pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>, // 追踪页表占用的物理页帧，自动回收
}

impl PageTable {
    pub fn new() -> Self {
        let frame = frame_alloc().expect("Failed to allocate frame for page table");
        Self {
            root_ppn: frame.ppn(),
            frames: vec![frame],
        }
    }
}

/// 页表设置页表项接口
/// 
/// 均返回页表项的可变引用，用于读取或设置页表项
impl PageTable {
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for i in 0..3 {
            let pte = &mut ppn.get_pte_array()[idxs[i]];
            if i == 2 {
                result = Some(pte); // 直接返回最终的页表项，不保证有效
                break;
            }
            if !pte.is_valid() {
                // 创建新的页表页
                let frame = frame_alloc().unwrap(); // 帧的分配保证页帧内页表项为空
                *pte = PageTableEntry::new(frame.ppn(), PTEFlags::VALID);
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        result
    }

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
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();
        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn); // 页表项应当没有被创建
        *pte = PageTableEntry::new(ppn, flags);
    }
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
}

/// 手动查询页表接口
impl PageTable {
    pub fn from_token(stap: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from(stap & ((1usize << 44) - 1)), // 从 stap 中得到物理地址
            frames: Vec::new(),
        }
    }

    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn)
            .map(|pte| {pte.clone()}) // TODO: 不太懂
    }
}
