// os/src/mm/address.rs

use core::fmt::{Debug, Formatter, self};
use crate::config::{PAGE_SIZE, PAGE_SIZE_BITS};
use super::page_table::PageTableEntry;

// 使用 sv39 页表模式
pub(super) const PA_WIDTH_SV39: usize = 56;
pub(super) const VA_WIDTH_SV39: usize = 39;
pub(super) const PPN_WIDTH_SV39: usize = PA_WIDTH_SV39 - PAGE_SIZE_BITS;
pub(super) const VPN_WIDTH_SV39: usize = VA_WIDTH_SV39 - PAGE_SIZE_BITS;

/// 物理地址
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub usize);

/// 虚拟地址
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub usize);

/// 物理页号
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysPageNum(pub usize);

/// 虚拟页号
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtPageNum(pub usize);

/// DEBUG
impl Debug for VirtAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("VA:{:#x}", self.0))
    }
}
impl Debug for VirtPageNum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("VPN:{:#x}", self.0))
    }
}
impl Debug for PhysAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("PA:{:#x}", self.0))
    }
}
impl Debug for PhysPageNum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("PPN:{:#x}", self.0))
    }
}


impl From<usize> for PhysAddr {
    fn from(value: usize) -> Self { Self(value & ((1 << PA_WIDTH_SV39) - 1)) }
}
impl From<usize> for PhysPageNum {
    fn from(value: usize) -> Self { Self(value & ((1 << PPN_WIDTH_SV39) - 1)) }
}

impl From<PhysAddr> for usize {
    fn from(value: PhysAddr) -> Self { value.0 }
}
impl From<PhysPageNum> for usize {
    fn from(value: PhysPageNum) -> Self { value.0 }
}


impl From<PhysPageNum> for PhysAddr {
    fn from(value: PhysPageNum) -> Self { Self(value.0 << PAGE_SIZE_BITS) }
}
impl From<PhysAddr> for PhysPageNum {
    fn from(value: PhysAddr) -> Self {
        assert_eq!(value.page_offset(), 0);
        value.floor()
    }
}

impl PhysAddr {
    pub fn page_offset(self) -> usize { self.0 & (PAGE_SIZE - 1) }

    pub fn floor(self) -> PhysPageNum { PhysPageNum(self.0 >> PAGE_SIZE_BITS) }
    pub fn ceil(self) -> PhysPageNum { PhysPageNum((self.0 + PAGE_SIZE - 1) >> PAGE_SIZE_BITS) }
}

impl PhysPageNum {
    pub fn get_pte_array(&self) -> &'static mut [PageTableEntry] {
        let pa = PhysAddr::from(*self);
        unsafe { core::slice::from_raw_parts_mut(pa.0 as *mut PageTableEntry, PAGE_SIZE / core::mem::size_of::<PageTableEntry>()) }
    }
}


impl VirtPageNum {
    pub fn indexes(&self) -> [usize; 3] {
        [
            (self.0 >> 12) & 0x1FF, // VPN[0]
            (self.0 >> 21) & 0x1FF, // VPN[1]
            (self.0 >> 30) & 0x1FF, // VPN[2]
        ]
    }
}
