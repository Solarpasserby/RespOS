// os/src/arch/loongarch64/mm/page_table.rs
//
// LoongArch LA64 页表实现 (3级页表, 4KB 页)
//
// 地址结构 (39-bit VA):
//   | PGD(9) | PMD(9) | PTE(9) | Offset(12) |
//   | 38..30 | 29..21 | 20..12 | 11..0      |
//
// PTE 格式 (64-bit):
//   | RPLV/NX/NR | Reserved | PPN[47:12] | Software | G/MAT/PLV/D/V |
//
// Flags:
//   bit 0: V (Valid)
//   bit 1: D (Dirty / Writable)
//   bits[3:2]: PLV (Privilege: 0=kernel, 3=user)
//   bits[5:4]: MAT (Memory type: 1=cached)
//   bit 6: G (Global)
//   bit 7: P (software present)
//   bit 8: W (software writable)
//   bit 61: NR (No Read)
//   bit 62: NX (No Execute)
//   bit 63: RPLV

use crate::config::{KERNEL_BASE, PAGE_SIZE_BITS};
use crate::mm::{FrameTracker, frame_alloc as alloc_frame};
use crate::mm::{
    KERNEL_SPACE, MapPermission, PPN_WIDTH, PhysAddr, PhysPageNum, VirtAddr, VirtPageNum,
};
use crate::syscall::{Errno, SysResult};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;
use lazy_static::lazy_static;
use spin::Mutex;

const PTE_V: usize = 1 << 0;
const PTE_D: usize = 1 << 1;
const PTE_PLV_USER: usize = 3 << 2;
const PTE_MAT_CC: usize = 1 << 4;
const PTE_G: usize = 1 << 6;
const PTE_P: usize = 1 << 7;
const PTE_W: usize = 1 << 8;
const PTE_COW: usize = 1 << 9;
const PTE_NR: usize = 1usize << 61;
const PTE_NX: usize = 1usize << 62;
const PTE_PPN_MASK: usize = ((1usize << PPN_WIDTH) - 1) << 12;

const PAGE_TABLE_FRAME_QUARANTINE_LIMIT: usize = 128;

lazy_static! {
    static ref PAGE_TABLE_FRAME_QUARANTINE: Mutex<PageTableFrameQuarantine> =
        Mutex::new(PageTableFrameQuarantine::new());
}

struct PageTableFrameQuarantine {
    page_count: usize,
    retired: VecDeque<Vec<FrameTracker>>,
}

impl PageTableFrameQuarantine {
    fn new() -> Self {
        Self {
            page_count: 0,
            retired: VecDeque::new(),
        }
    }

    fn retire(&mut self, frames: Vec<FrameTracker>) -> Vec<Vec<FrameTracker>> {
        if frames.is_empty() {
            return Vec::new();
        }

        self.page_count += frames.len();
        self.retired.push_back(frames);

        let mut expired = Vec::new();
        while self.page_count > PAGE_TABLE_FRAME_QUARANTINE_LIMIT {
            let Some(frames) = self.retired.pop_front() else {
                self.page_count = 0;
                break;
            };
            self.page_count -= frames.len();
            expired.push(frames);
        }
        expired
    }
}

pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
}

impl PageTable {
    pub fn new() -> Self {
        let frame = alloc_frame().expect("Failed to allocate frame for page table");
        Self {
            root_ppn: frame.ppn(),
            frames: vec![frame],
        }
    }

    pub fn from_kernel() -> SysResult<Self> {
        let frame = alloc_frame().ok_or(Errno::ENOMEM)?;
        let kernel_page_table = &KERNEL_SPACE.lock().page_table;
        let kernel_root_ppn = kernel_page_table.root_ppn;
        let pgd_idx = (KERNEL_BASE >> (PAGE_SIZE_BITS + 18)) & 0x1FF;
        let dst = frame.ppn().get_pte_array();
        let src = kernel_root_ppn.get_pte_array();
        dst[pgd_idx..].copy_from_slice(&src[pgd_idx..]);
        Ok(PageTable {
            root_ppn: frame.ppn(),
            frames: vec![frame],
        })
    }

    pub fn from_token(token: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from((token >> 12) & ((1usize << PPN_WIDTH) - 1)),
            frames: Vec::new(),
        }
    }

    pub fn token(&self) -> usize {
        self.root_ppn.0 << 12
    }

    /// 延迟回收当前页表持有的页表页帧。
    ///
    /// LoongArch release 下，短进程密集退出时立刻回收并复用页表页帧会触发
    /// 上下文切换后的卡死。这里把页表页帧放进有限隔离队列，避免立即复用，
    /// 队列超过上限后再释放最旧的一批，防止进程数量增长时无限占用内存。
    pub fn retire_owned_frames(&mut self) {
        let expired = PAGE_TABLE_FRAME_QUARANTINE
            .lock()
            .retire(core::mem::take(&mut self.frames));
        drop(expired);
    }

    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }

    pub fn translate_va(&self, va: VirtAddr) -> Option<PhysAddr> {
        self.find_pte(va.floor()).map(|pte| {
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

fn get_vpn_indexes(vpn: VirtPageNum) -> [usize; 3] {
    let v = vpn.0;
    [
        (v >> 18) & 0x1FF, // PGD: VA[38:30]
        (v >> 9) & 0x1FF,  // PMD: VA[29:21]
        v & 0x1FF,         // PTE: VA[20:12]
    ]
}

impl PageTable {
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> SysResult<&mut PageTableEntry> {
        let idxs = get_vpn_indexes(vpn);
        let mut ppn = self.root_ppn;
        for i in 0..3 {
            let pte = &mut ppn.get_pte_array()[idxs[i]];
            if i == 2 {
                return Ok(pte);
            }
            if !pte.is_valid() {
                let frame = alloc_frame().ok_or(Errno::ENOMEM)?;
                *pte = PageTableEntry::new_table(frame.ppn());
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        Err(Errno::EFAULT)
    }

    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = get_vpn_indexes(vpn);
        let mut ppn = self.root_ppn;
        for i in 0..3 {
            let pte = &mut ppn.get_pte_array()[idxs[i]];
            if i == 2 {
                return Some(pte);
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        None
    }
}

impl PageTable {
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) -> SysResult {
        let pte = self.find_pte_create(vpn)?;
        if pte.is_valid() {
            return Err(Errno::EEXIST);
        }
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::VALID | PTEFlags::ACCESSED);
        Ok(())
    }

    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
    }

    pub fn try_unmap(&mut self, vpn: VirtPageNum) {
        let pte = match self.find_pte(vpn) {
            Some(pte) => pte,
            None => return,
        };
        if !pte.is_valid() {
            return;
        }
        *pte = PageTableEntry::empty();
    }

    pub fn modify_pte(&mut self, vpn: VirtPageNum, flags: PTEFlags) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid in modify_pte", vpn);
        *pte = PageTableEntry::new(pte.ppn(), flags | PTEFlags::VALID | PTEFlags::ACCESSED);
    }

    pub fn set_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        pte.set_cow_bit();
    }

    pub fn make_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid in make_pte_cow", vpn);
        pte.make_cow();
    }

    pub fn clear_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        pte.clear_cow_bit();
    }
}

/// LoongArch 页表项
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize,
}

bitflags! {
    pub struct PTEFlags: u16 {
        const VALID    = 1 << 0;
        const READ     = 1 << 1;  // maps to !NR via conversion
        const WRITE    = 1 << 2;  // maps to D via conversion
        const EXECUTE  = 1 << 3;  // maps to !NX via conversion
        const USER     = 1 << 4;
        const GLOBAL   = 1 << 5;
        const ACCESSED = 1 << 6;  // LA64 has no explicit A bit, kept for interface
        const DIRTY    = 1 << 7;  // maps to D bit
        const COW      = 1 << 9;  // Copy-on-Write (software flag)
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
        if self.contains(PTEFlags::COW) {
            ret.push_str("COW");
        }
        ret
    }
}

impl From<MapPermission> for PTEFlags {
    fn from(value: MapPermission) -> Self {
        PTEFlags::from_bits(value.bits()).unwrap()
    }
}

/// 将通用 PTEFlags 转换为 LoongArch PTE bits。
fn flags_to_la64(flags: PTEFlags) -> usize {
    let mut la64: usize = 0;
    if flags.contains(PTEFlags::VALID) {
        la64 |= PTE_V;
    }
    if flags.contains(PTEFlags::DIRTY) || flags.contains(PTEFlags::WRITE) {
        la64 |= PTE_D;
    }
    if flags.contains(PTEFlags::USER) {
        la64 |= PTE_PLV_USER;
    }
    la64 |= PTE_MAT_CC;
    if flags.contains(PTEFlags::GLOBAL) {
        la64 |= PTE_G;
    }
    if flags.contains(PTEFlags::VALID) {
        la64 |= PTE_P;
    }
    if flags.contains(PTEFlags::WRITE) {
        la64 |= PTE_W;
    }
    if flags.contains(PTEFlags::COW) {
        la64 |= PTE_COW;
    }
    if !flags.contains(PTEFlags::READ) {
        la64 |= PTE_NR;
    }
    if !flags.contains(PTEFlags::EXECUTE) {
        la64 |= PTE_NX;
    }
    la64
}

/// 将 LoongArch PTE bits 转换为通用 PTEFlags。
fn flags_from_la64(bits: usize) -> PTEFlags {
    let mut flags = PTEFlags::empty();
    if bits & PTE_V != 0 {
        flags |= PTEFlags::VALID;
    }
    if bits & PTE_W != 0 {
        flags |= PTEFlags::WRITE;
    }
    if bits & PTE_D != 0 {
        flags |= PTEFlags::DIRTY;
    }
    if bits & PTE_NR == 0 {
        flags |= PTEFlags::READ;
    }
    if bits & PTE_NX == 0 {
        flags |= PTEFlags::EXECUTE;
    }
    if (bits >> 2) & 3 == 3 {
        flags |= PTEFlags::USER;
    }
    if bits & PTE_G != 0 {
        flags |= PTEFlags::GLOBAL;
    }
    if bits & PTE_COW != 0 {
        flags |= PTEFlags::COW;
    }
    flags | PTEFlags::ACCESSED
}

impl PageTableEntry {
    pub fn new_table(ppn: PhysPageNum) -> Self {
        Self {
            bits: (ppn.0 << 12) | PTE_V,
        }
    }

    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        Self {
            bits: (ppn.0 << 12) | flags_to_la64(flags),
        }
    }

    pub fn empty() -> Self {
        Self { bits: 0 }
    }

    pub fn ppn(&self) -> PhysPageNum {
        PhysPageNum((self.bits & PTE_PPN_MASK) >> 12)
    }

    pub fn flags(&self) -> PTEFlags {
        flags_from_la64(self.bits & !PTE_PPN_MASK)
    }

    pub fn is_valid(&self) -> bool {
        self.bits & PTE_V != 0
    }

    pub fn readable(&self) -> bool {
        self.bits & PTE_NR == 0
    }

    pub fn writable(&self) -> bool {
        self.bits & PTE_D != 0
    }

    pub fn executable(&self) -> bool {
        self.bits & PTE_NX == 0
    }

    /// COW 标志存储在软件位 [9]。
    pub fn is_cow(&self) -> bool {
        self.bits & PTE_COW != 0
    }

    pub fn set_cow_bit(&mut self) {
        self.bits &= !(PTE_W | PTE_D);
        self.bits |= PTE_COW;
    }

    pub fn make_cow(&mut self) {
        self.bits &= !(PTE_W | PTE_D);
        self.bits |= PTE_COW | PTE_P | PTE_V;
    }

    pub fn clear_cow_bit(&mut self) {
        self.bits &= !PTE_COW;
    }
}
