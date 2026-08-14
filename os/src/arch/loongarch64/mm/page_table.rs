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
//   bit 10: PROTNONE (software-present leaf with no hardware access)
//   bit 61: NR (No Read)
//   bit 62: NX (No Execute)
//   bit 63: RPLV

use crate::config::{KERNEL_BASE, PAGE_SIZE_BITS};
use crate::mm::{FrameTracker, frame_alloc as alloc_frame};
use crate::mm::{
    KERNEL_SPACE, MapPermission, PPN_WIDTH, PhysAddr, PhysPageNum, VirtAddr, VirtPageNum,
};
use crate::syscall::{Errno, SysResult};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;
use spin::Mutex;

const PTE_V: usize = 1 << 0;
const PTE_D: usize = 1 << 1;
const PTE_PLV_USER: usize = 3 << 2;
const PTE_MAT_CC: usize = 1 << 4;
const PTE_G: usize = 1 << 6;
const PMD_HUGE: usize = 1 << 6;
const PTE_P: usize = 1 << 7;
const PTE_W: usize = 1 << 8;
const PTE_COW: usize = 1 << 9;
const PTE_PROTNONE: usize = 1 << 10;
const PTE_NR: usize = 1usize << 61;
const PTE_NX: usize = 1usize << 62;
const PTE_PPN_MASK: usize = ((1usize << PPN_WIDTH) - 1) << 12;

struct PendingTlbRange {
    start_vpn: usize,
    end_vpn: usize,
}

impl PendingTlbRange {
    const fn empty() -> Self {
        Self {
            start_vpn: usize::MAX,
            end_vpn: 0,
        }
    }
}

pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
    // Data frames whose PTEs were removed or replaced. Keep them with the
    // owning address space until its synchronous shootdown completes; a
    // process-independent retirement queue cannot safely choose a target
    // hart set for a batch containing multiple ASIDs.
    retired_data_frames: Vec<Arc<FrameTracker>>,
    // Page-table frames cannot be reused while an exiting task still runs on
    // this root. The last active-hart transition releases this owned batch.
    retired_page_table_frames: Mutex<Vec<FrameTracker>>,
    // Conservative half-open VPN envelope covering every successful leaf
    // PTE mutation since the previous synchronous flush. The envelope may
    // include untouched pages between sparse changes, but never omits one.
    // &mut PTE writers use get_mut(); activate() only needs interior reset.
    pending_tlb_range: Mutex<PendingTlbRange>,
}

impl PageTable {
    /// Mark the already-built, immutable kernel half as global before its
    /// first activation. LoongArch 4 KiB TLB entries cover an even/odd pair;
    /// the entry is global only when both TLBELO G bits are set. Leave an
    /// unpaired leaf non-global rather than claiming a property the hardware
    /// will not provide. Runtime mappings such as recycled kernel stacks are
    /// installed after this pass and deliberately remain ASID-scoped.
    #[cfg(feature = "la_global_kernel")]
    pub fn mark_existing_kernel_global(&mut self) -> (usize, usize) {
        let kernel_pgd = (KERNEL_BASE >> (PAGE_SIZE_BITS + 18)) & 0x1ff;
        let mut global_pairs = 0;
        let mut skipped_huge = 0;
        let root = self.root_ppn.get_pte_array();

        for pgd in &mut root[kernel_pgd..] {
            if !pgd.is_valid() {
                continue;
            }
            assert!(!pgd.is_huge(), "unexpected 1 GiB LoongArch kernel leaf");
            let pmds = pgd.ppn().get_pte_array();
            for pmd in pmds {
                if !pmd.is_valid() {
                    continue;
                }
                if pmd.is_huge() {
                    skipped_huge += 1;
                    continue;
                }

                let ptes = pmd.ppn().get_pte_array();
                for pair in ptes.chunks_exact_mut(2) {
                    if pair[0].is_valid() && pair[1].is_valid() {
                        pair[0].bits |= PTE_G;
                        pair[1].bits |= PTE_G;
                        global_pairs += 1;
                    } else {
                        pair[0].bits &= !PTE_G;
                        pair[1].bits &= !PTE_G;
                    }
                }
            }
        }

        (global_pairs, skipped_huge)
    }

    /// Install one aligned 2 MiB kernel direct-map entry in the PMD.
    ///
    /// Global huge-leaf encoding is deliberately rejected. The previous
    /// bit-12 assumption faulted immediately when the high-RAM direct map was
    /// exercised; 4 KiB paired global leaves are evaluated independently.
    pub fn map_huge_2m(&mut self, va: VirtAddr, pa: PhysAddr, flags: PTEFlags) -> SysResult {
        const HUGE_SIZE: usize = 2 * 1024 * 1024;
        let va_raw = usize::from(va);
        let pa_raw = usize::from(pa);
        if va_raw % HUGE_SIZE != 0 || pa_raw % HUGE_SIZE != 0 || flags.contains(PTEFlags::GLOBAL) {
            return Err(Errno::EINVAL);
        }

        let indexes = get_vpn_indexes(VirtPageNum::from(va_raw >> PAGE_SIZE_BITS));
        let root = &mut self.root_ppn.get_pte_array()[indexes[0]];
        if !root.is_valid() {
            let frame = alloc_frame().ok_or(Errno::ENOMEM)?;
            *root = PageTableEntry::new_table(frame.ppn());
            self.frames.push(frame);
        }
        if root.is_huge() {
            return Err(Errno::EEXIST);
        }

        let pmd = &mut root.ppn().get_pte_array()[indexes[1]];
        if pmd.is_valid() {
            return Err(Errno::EEXIST);
        }
        let mut bits = (pa_raw >> PAGE_SIZE_BITS) << 12;
        bits |= flags_to_la64(flags | PTEFlags::VALID | PTEFlags::ACCESSED);
        pmd.bits = bits | PMD_HUGE;
        self.record_tlb_range(VirtPageNum::from(va_raw >> PAGE_SIZE_BITS), 512);
        Ok(())
    }

    /// Populate every still-empty kernel-half root entry before user roots
    /// copy the kernel half.  Later dynamic mappings then only mutate shared
    /// lower-level tables, so an old user root cannot miss a newly-created
    /// kernel-stack root branch.
    pub fn prepare_kernel_root_branches(&mut self) -> SysResult {
        let first_kernel_index = (KERNEL_BASE >> (PAGE_SIZE_BITS + 18)) & 0x1ff;
        for index in first_kernel_index..512 {
            let root = &mut self.root_ppn.get_pte_array()[index];
            if root.is_valid() {
                continue;
            }
            let frame = alloc_frame().ok_or(Errno::ENOMEM)?;
            *root = PageTableEntry::new_table(frame.ppn());
            self.frames.push(frame);
        }
        Ok(())
    }

    pub fn new() -> Self {
        let frame = alloc_frame().expect("Failed to allocate frame for page table");
        Self {
            root_ppn: frame.ppn(),
            frames: vec![frame],
            retired_data_frames: Vec::new(),
            retired_page_table_frames: Mutex::new(Vec::new()),
            pending_tlb_range: Mutex::new(PendingTlbRange::empty()),
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
            retired_data_frames: Vec::new(),
            retired_page_table_frames: Mutex::new(Vec::new()),
            pending_tlb_range: Mutex::new(PendingTlbRange::empty()),
        })
    }

    pub fn from_token(token: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from((token >> 12) & ((1usize << PPN_WIDTH) - 1)),
            frames: Vec::new(),
            retired_data_frames: Vec::new(),
            retired_page_table_frames: Mutex::new(Vec::new()),
            pending_tlb_range: Mutex::new(PendingTlbRange::empty()),
        }
    }

    pub fn token(&self) -> usize {
        self.root_ppn.0 << 12
    }

    pub fn retire_data_frame(&mut self, frame: Arc<FrameTracker>) {
        self.retired_data_frames.push(frame);
    }

    pub fn take_retired_data_frames(&mut self) -> Vec<Arc<FrameTracker>> {
        core::mem::take(&mut self.retired_data_frames)
    }

    fn record_tlb_range(&mut self, start: VirtPageNum, pages: usize) {
        if pages == 0 {
            return;
        }
        let end = start.0.checked_add(pages).expect("TLB range VPN overflow");
        let pending = self.pending_tlb_range.get_mut();
        pending.start_vpn = pending.start_vpn.min(start.0);
        pending.end_vpn = pending.end_vpn.max(end);
    }

    fn record_tlb_change(&mut self, vpn: VirtPageNum) {
        self.record_tlb_range(vpn, 1);
    }

    pub fn take_pending_tlb_range(&mut self) -> Option<(usize, usize)> {
        let pending = self.pending_tlb_range.get_mut();
        if pending.start_vpn == usize::MAX {
            debug_assert_eq!(pending.end_vpn, 0);
            return None;
        }
        let start_vpn = pending.start_vpn;
        let end_vpn = pending.end_vpn;
        *pending = PendingTlbRange::empty();
        debug_assert!(start_vpn < end_vpn);
        Some((
            usize::from(VirtAddr::from(VirtPageNum(start_vpn))),
            usize::from(VirtAddr::from(VirtPageNum(end_vpn))),
        ))
    }

    pub fn discard_pending_tlb_range(&self) {
        let mut pending = self.pending_tlb_range.lock();
        *pending = PendingTlbRange::empty();
    }

    pub fn retire_owned_frames(&mut self) {
        let frames = core::mem::take(&mut self.frames);
        if frames.is_empty() {
            return;
        }
        let mut retired = self.retired_page_table_frames.lock();
        debug_assert!(retired.is_empty(), "page-table frames retired twice");
        *retired = frames;
    }

    pub fn release_retired_page_table_frames(&self) {
        self.retired_page_table_frames.lock().clear();
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
        self.record_tlb_change(vpn);
        Ok(())
    }

    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
        self.record_tlb_change(vpn);
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
        self.record_tlb_change(vpn);
    }

    pub fn modify_pte(&mut self, vpn: VirtPageNum, flags: PTEFlags) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid in modify_pte", vpn);
        *pte = PageTableEntry::new(pte.ppn(), flags | PTEFlags::VALID | PTEFlags::ACCESSED);
        self.record_tlb_change(vpn);
    }

    /// Atomically replace an existing leaf mapping without allocating page-table
    /// pages. Used by COW after the replacement frame has been fully prepared.
    pub fn replace_pte(
        &mut self,
        vpn: VirtPageNum,
        ppn: PhysPageNum,
        flags: PTEFlags,
    ) -> SysResult {
        let pte = self.find_pte(vpn).ok_or(Errno::EFAULT)?;
        if !pte.is_valid() {
            return Err(Errno::EFAULT);
        }
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::VALID | PTEFlags::ACCESSED);
        self.record_tlb_change(vpn);
        Ok(())
    }

    pub fn set_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        pte.set_cow_bit();
        self.record_tlb_change(vpn);
    }

    pub fn make_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid in make_pte_cow", vpn);
        pte.make_cow();
        self.record_tlb_change(vpn);
    }

    pub fn clear_pte_cow(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        pte.clear_cow_bit();
        self.record_tlb_change(vpn);
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
    let prot_none = flags.contains(PTEFlags::USER)
        && !flags.intersects(PTEFlags::READ | PTEFlags::WRITE | PTEFlags::EXECUTE);
    if flags.contains(PTEFlags::VALID) && !prot_none {
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
    if flags.contains(PTEFlags::VALID) && prot_none {
        // QEMU 10.0.2 masks NR/NX out while executing LDPTE. Keep the leaf
        // software-present but hardware-invalid so PROT_NONE remains
        // inaccessible there as well as on hardware/newer emulators.
        la64 |= PTE_PROTNONE;
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
    if bits & (PTE_V | PTE_PROTNONE) != 0 {
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
        // Callers use is_valid() as the resident/software-present predicate.
        // PROT_NONE leaves deliberately clear hardware V but must still be
        // found by mprotect(), munmap(), and fork().
        self.bits & (PTE_V | PTE_PROTNONE) != 0
    }

    fn is_huge(&self) -> bool {
        self.bits & PMD_HUGE != 0
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
