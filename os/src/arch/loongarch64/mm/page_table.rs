// os/src/arch/loongarch64/mm/page_table.rs
//
// LoongArch LA64 页表实现 (4级页表, 4KB 页)
//
// 地址结构 (48-bit VA):
//   | PGD(9) | PUD(9) | PMD(9) | PTE(9) | Offset(12) |
//   | 47..39 | 38..30 | 29..21 | 20..12 | 11..0      |
//
// PTE 格式 (64-bit):
//   | Reserved[63:48] | PPN[47:12] | Flags[11:0] |
//
// Flags:
//   bit 0: V (Valid)
//   bit 1: D (Dirty)
//   bits[3:2]: PLV (Privilege: 0=kernel, 3=user)
//   bits[5:4]: MAT (Memory type: 1=cached)
//   bit 6: G (Global)
//   bit 7: NR (No Read)
//   bit 8: NX (No Execute)
//   bits[11:9]: RPLV

use crate::config::KERNEL_BASE;
use crate::mm::{
    KERNEL_SPACE, PhysAddr, PhysPageNum, PPN_WIDTH, VirtAddr, VirtPageNum,
};
use crate::mm::{FrameTracker, frame_alloc as alloc_frame};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;

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

    pub fn from_kernel() -> Self {
        let frame = alloc_frame().unwrap();
        let kernel_page_table = &KERNEL_SPACE.lock().page_table;
        let kernel_root_ppn = kernel_page_table.root_ppn;
        // 拷贝内核空间映射: 复制根页表的内核部分
        // LA64 下 KERNEL_BASE 的 VPN[47:39] 决定了 PGD 索引
        let vpn = VirtPageNum::from(VirtAddr::from(KERNEL_BASE).floor().0);
        let pgd_idx = (vpn.0 >> 27) & 0x1FF;
        // 从 pgd_idx 到 512 之间的条目属于内核空间
        let dst = frame.ppn().get_pte_array();
        let src = kernel_root_ppn.get_pte_array();
        dst[pgd_idx..].copy_from_slice(&src[pgd_idx..]);
        PageTable {
            root_ppn: frame.ppn(),
            frames: vec![frame],
        }
    }

    pub fn from_token(token: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from((token >> 12) & ((1usize << PPN_WIDTH) - 1)),
            frames: Vec::new(),
        }
    }

    pub fn token(&self) -> usize {
        // PGDH 格式: root_ppn << 12
        self.root_ppn.0 << 12
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

/// LA64 4级页表 VPN 索引
fn get_vpn_indexes(vpn: VirtPageNum) -> [usize; 4] {
    let v = vpn.0;
    [
        (v >> 27) & 0x1FF, // PGD: VA[47:39]
        (v >> 18) & 0x1FF, // PUD: VA[38:30]
        (v >> 9) & 0x1FF,  // PMD: VA[29:21]
        v & 0x1FF,         // PTE: VA[20:12]
    ]
}

impl PageTable {
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = get_vpn_indexes(vpn);
        let mut ppn = self.root_ppn;
        for i in 0..4 {
            let pte = &mut ppn.get_pte_array()[idxs[i]];
            if i == 3 {
                return Some(pte);
            }
            if !pte.is_valid() {
                let frame = alloc_frame().unwrap();
                *pte = PageTableEntry::new(frame.ppn(), PTEFlags::VALID);
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        None
    }

    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = get_vpn_indexes(vpn);
        let mut ppn = self.root_ppn;
        for i in 0..4 {
            let pte = &mut ppn.get_pte_array()[idxs[i]];
            if i == 3 {
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
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();
        assert!(
            !pte.is_valid(),
            "vpn {:?} is mapped before mapping",
            vpn
        );
        *pte = PageTableEntry::new(
            ppn,
            flags | PTEFlags::VALID | PTEFlags::ACCESSED | PTEFlags::DIRTY,
        );
    }

    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
    }
}

/// LoongArch 页表项
///
/// | Reserved[63:48] | PPN[47:12] | RPLV[11:9] | NX | NR | G | MAT[5:4] | PLV[3:2] | D | V |
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize,
}

bitflags! {
    pub struct PTEFlags: u8 {
        const VALID    = 1 << 0;
        const READ     = 1 << 1;  // maps to !NR via conversion
        const WRITE    = 1 << 2;  // maps to D via conversion
        const EXECUTE  = 1 << 3;  // maps to !NX via conversion
        const USER     = 1 << 4;
        const GLOBAL   = 1 << 5;
        const ACCESSED = 1 << 6;  // LA64 has no explicit A bit, kept for interface
        const DIRTY    = 1 << 7;  // maps to D bit
    }
}

#[allow(unused)]
impl PTEFlags {
    pub fn readable_flags(&self) -> String {
        let mut ret = String::new();
        if self.contains(PTEFlags::VALID) { ret.push_str("V"); }
        if self.contains(PTEFlags::READ) { ret.push_str("R"); }
        if self.contains(PTEFlags::WRITE) { ret.push_str("W"); }
        if self.contains(PTEFlags::EXECUTE) { ret.push_str("X"); }
        if self.contains(PTEFlags::USER) { ret.push_str("U"); }
        if self.contains(PTEFlags::GLOBAL) { ret.push_str("G"); }
        if self.contains(PTEFlags::ACCESSED) { ret.push_str("A"); }
        if self.contains(PTEFlags::DIRTY) { ret.push_str("D"); }
        ret
    }
}

/// 将 RISC-V 风格的 PTEFlags 转换为 LoongArch PTE bits[11:0]
fn flags_to_la64(flags: PTEFlags) -> usize {
    let mut la64: usize = 0;
    if flags.contains(PTEFlags::VALID) {
        la64 |= 1 << 0; // V
    }
    if flags.contains(PTEFlags::DIRTY) || flags.contains(PTEFlags::WRITE) {
        la64 |= 1 << 1; // D
    }
    // PLV: USER → 3, otherwise → 0
    if flags.contains(PTEFlags::USER) {
        la64 |= 3 << 2;
    }
    // MAT: always coherent cached (1)
    la64 |= 1 << 4;
    if flags.contains(PTEFlags::GLOBAL) {
        la64 |= 1 << 6;
    }
    // NR: No Read when READ flag absent
    if !flags.contains(PTEFlags::READ) {
        la64 |= 1 << 7;
    }
    // NX: No Execute when EXECUTE flag absent
    if !flags.contains(PTEFlags::EXECUTE) {
        la64 |= 1 << 8;
    }
    la64
}

/// 将 LoongArch PTE bits[11:0] 转换为 RISC-V 风格的 PTEFlags
fn flags_from_la64(bits: usize) -> PTEFlags {
    let mut flags = PTEFlags::empty();
    if bits & (1 << 0) != 0 {
        flags |= PTEFlags::VALID;
    }
    if bits & (1 << 1) != 0 {
        flags |= PTEFlags::DIRTY | PTEFlags::WRITE;
    }
    if bits & (1 << 7) == 0 {
        flags |= PTEFlags::READ;
    }
    if bits & (1 << 8) == 0 {
        flags |= PTEFlags::EXECUTE;
    }
    if (bits >> 2) & 3 == 3 {
        flags |= PTEFlags::USER;
    }
    if bits & (1 << 6) != 0 {
        flags |= PTEFlags::GLOBAL;
    }
    flags | PTEFlags::ACCESSED
}

impl PageTableEntry {
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        Self {
            bits: (ppn.0 << 12) | flags_to_la64(flags),
        }
    }

    pub fn empty() -> Self {
        Self { bits: 0 }
    }

    pub fn ppn(&self) -> PhysPageNum {
        PhysPageNum((self.bits >> 12) & ((1usize << PPN_WIDTH) - 1))
    }

    pub fn flags(&self) -> PTEFlags {
        flags_from_la64(self.bits & 0xFFF)
    }

    pub fn is_valid(&self) -> bool {
        self.bits & (1 << 0) != 0
    }

    pub fn readable(&self) -> bool {
        self.bits & (1 << 7) == 0
    }

    pub fn writable(&self) -> bool {
        self.bits & (1 << 1) != 0
    }

    pub fn executable(&self) -> bool {
        self.bits & (1 << 8) == 0
    }
}
