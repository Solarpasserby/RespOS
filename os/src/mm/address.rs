// os/src/mm/address.rs

use core::fmt::{Debug, Formatter, self};
use crate::config::{PAGE_SIZE, PAGE_SIZE_BITS};

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
impl From<usize> for VirtAddr {
    fn from(value: usize) -> Self { Self(value & ((1 << VA_WIDTH_SV39) - 1)) }
}
impl From<usize> for VirtPageNum {
    fn from(value: usize) -> Self { Self(value & ((1 << VPN_WIDTH_SV39) - 1)) }
}

impl From<PhysAddr> for usize {
    fn from(value: PhysAddr) -> Self { value.0 }
}
impl From<PhysPageNum> for usize {
    fn from(value: PhysPageNum) -> Self { value.0 }
}
impl From<VirtAddr> for usize {
    fn from(v: VirtAddr) -> Self {
        // 实现符号拓展
        if v.0 >= (1 << (VA_WIDTH_SV39 - 1)) {
            v.0 | (!((1 << VA_WIDTH_SV39) - 1))
        } else {
            v.0
        }
    }
}
impl From<VirtPageNum> for usize {
    fn from(value: VirtPageNum) -> Self { value.0 }
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
impl From<VirtPageNum> for VirtAddr {
    fn from(v: VirtPageNum) -> Self { Self(v.0 << PAGE_SIZE_BITS) }
}
impl From<VirtAddr> for VirtPageNum {
    fn from(v: VirtAddr) -> Self {
        assert_eq!(v.page_offset(), 0);
        v.floor()
    }
}

impl PhysAddr {
    pub fn page_offset(self) -> usize { self.0 & (PAGE_SIZE - 1) }

    pub fn floor(self) -> PhysPageNum { PhysPageNum(self.0 >> PAGE_SIZE_BITS) }
    pub fn ceil(self) -> PhysPageNum { PhysPageNum((self.0 + PAGE_SIZE - 1) >> PAGE_SIZE_BITS) }
}
impl VirtAddr {
    pub fn page_offset(&self) -> usize { self.0 & (PAGE_SIZE - 1) }

    pub fn floor(&self) -> VirtPageNum { VirtPageNum(self.0 >> PAGE_SIZE_BITS) }
    pub fn ceil(&self) -> VirtPageNum { VirtPageNum((self.0 + PAGE_SIZE - 1) >> PAGE_SIZE_BITS) }
}

impl VirtPageNum {
    pub fn indexes(&self) -> [usize; 3] {
        [
            (self.0 >> 18) & 0x1FF, // VPN[0]
            (self.0 >> 9) & 0x1FF,  // VPN[1]
            self.0 & 0x1FF,         // VPN[2]
        ]
    }
}


impl StepByOne for VirtPageNum {
    fn step(&mut self) {
        self.0 += 1;
    }
}

/// 步进特征
pub trait StepByOne {
    fn step(&mut self);
}

/// 简单范围
/// 
/// 主要用于描述一段范围
#[derive(Copy, Clone)]
pub struct SimpleRange<T>
where 
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    start: T,
    end: T,
}

impl<T> SimpleRange<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    pub fn new(start: T, end: T) -> Self {
        assert!(start <= end, "start {:?} > end {:?}!", start, end);
        Self { start, end }
    }
    pub fn get_start(&self) -> T { self.start }
    pub fn get_end(&self) -> T { self.end }
}

impl<T> IntoIterator for SimpleRange<T> // TODO: 对迭代器的了解不足，不清楚为什么要转移变量的所有权
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    type Item = T;
    type IntoIter = SimpleRangeIterator<T>;
    fn into_iter(self) -> Self::IntoIter {
        SimpleRangeIterator::new(self.start, self.end)
    }
}


pub struct SimpleRangeIterator<T>
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    current: T,
    end: T,
}
impl<T> SimpleRangeIterator<T> 
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    pub fn new(current: T, end: T) -> Self {
        Self { current, end }
    }
}

impl<T> Iterator for SimpleRangeIterator<T> 
where
    T: StepByOne + Copy + PartialEq + PartialOrd + Debug,
{
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        if self.current == self.end {
            None
        } else {
            let t = self.current;
            self.current.step();
            Some(t)
        }
    }
}


/// 虚拟页段
/// 
/// 主要用于描述一段连续的虚拟页表
pub type VPNRange = SimpleRange<VirtPageNum>;

pub fn get_bytes_array(ppn: PhysPageNum) -> &'static mut [u8] {
    let pa = PhysAddr::from(ppn);
    unsafe { core::slice::from_raw_parts_mut(pa.0 as *mut u8, crate::config::PAGE_SIZE) }
}
