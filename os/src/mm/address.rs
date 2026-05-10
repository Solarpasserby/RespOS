// os/src/mm/address.rs

use core::fmt::{Debug, Formatter, self};
use core::ops::Sub;
use crate::config::{KERNEL_BASE, PAGE_SIZE, PAGE_SIZE_BITS};
use super::PageTableEntry;

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

// 另一种地址表达，在转换时就检查地址有效性
// impl From<usize> for PhysAddr {
//     fn from(value: usize) -> Self {
//         assert!(value < (1 << PA_WIDTH_SV39));
//         Self(value) 
//     }
// }
// impl From<usize> for PhysPageNum {
//     fn from(value: usize) -> Self {
//         assert!(value < (1 << PPN_WIDTH_SV39));
//         Self(value) 
//     }
// }
// impl From<usize> for VirtAddr {
//     fn from(value: usize) -> Self {
//         let upper = (value as isize) >> VA_WIDTH_SV39;
//         assert!(upper == 0 || upper == -1, "[kernel] invalid sv39 virtual address: {:#x}", value);
//         Self(value)
//     }
// }
// impl From<usize> for VirtPageNum {
//     fn from(value: usize) -> Self {
//         assert!(value < (1 << VPN_WIDTH_SV39));
//         Self(value) 
//     }
// }

impl From<PhysAddr> for usize {
    fn from(value: PhysAddr) -> Self { value.0 }
}
impl From<PhysPageNum> for usize {
    fn from(value: PhysPageNum) -> Self { value.0 }
}
// TODO: 此处做了修改，之后某些地址越界的问题可能是这里导致的
impl From<VirtAddr> for usize {
    fn from(v: VirtAddr) -> Self { // 符号拓展
        (((v.0 << 25) as isize) >> 25) as usize
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

impl StepByOne for PhysPageNum {
    fn step(&mut self) {
        self.0 += 1;
    }
}
impl StepByOne for VirtPageNum {
    fn step(&mut self) {
        self.0 += 1;
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

impl PhysAddr {
    pub fn get_mut<T>(&self) -> &'static mut T {
        let ptr = (self.0 + KERNEL_BASE) as *mut T;
        unsafe { ptr.as_mut().unwrap() }
    }
    pub fn get_ref<T>(&self) -> &'static T {
        let ptr = (self.0 + KERNEL_BASE) as *const T;
        unsafe { ptr.as_ref().unwrap() }
    }
}
// 注意：通过 ppn 获取数据的方法仅限于数据存储于内核（待确认）；
//      裸指针指向的是虚拟地址，因此由于内核线性映射偏移的存在地址还需作额外转换
impl PhysPageNum {
    pub fn get_pte_array(&self) -> &'static mut [PageTableEntry] {
        let pa = PhysAddr::from(*self);
        let va = VirtAddr::from(pa.0 + KERNEL_BASE);
        let ptr = usize::from(va) as *mut PageTableEntry;
        unsafe { core::slice::from_raw_parts_mut(ptr, 512) }
    }
    pub fn get_bytes_array(&self) -> &'static mut [u8] {
        let pa = PhysAddr::from(*self);
        let va = VirtAddr::from(pa.0 + KERNEL_BASE);
        let ptr = usize::from(va) as *mut u8;
        unsafe { core::slice::from_raw_parts_mut(ptr, 4096) }
    }
    /// Get mutable reference to T on `PhysPageNum`
    pub fn get_mut<T>(&self) -> &'static mut T {
        let pa = PhysAddr::from(*self);
        let va = VirtAddr::from(pa.0 + KERNEL_BASE);
        let ptr = usize::from(va) as *mut T;
        unsafe { &mut *ptr }
    }
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

impl Sub for VirtPageNum {
    type Output = usize;

    fn sub(self, rhs: Self) -> Self::Output {
        let sign_bit = 1usize << (VPN_WIDTH_SV39 - 1);
        assert_eq!(
            self.0 & sign_bit,
            rhs.0 & sign_bit,
            "[kernel] virtual page subtraction requires pages in the same Sv39 half: {:?}, {:?}",
            self,
            rhs,
        );
        self.0
            .checked_sub(rhs.0)
            .expect("[kernel] virtual page subtraction underflow")
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
    
    pub fn contain(&self, pos: &T) -> bool {
        self.start <= *pos && *pos < self.end
    }
    pub fn contain_range(&self, other: &Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
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
