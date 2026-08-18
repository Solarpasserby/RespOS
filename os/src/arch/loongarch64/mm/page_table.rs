// os/src/arch/loongarch64/mm/page_table.rs
//
// LoongArch LA64 页表实现（三级页表，4 KiB 页）
//
// 地址结构（39-bit VA）：
//   | PGD(9) | PMD(9) | PTE(9) | 页内偏移(12) |
//   | 38..30 | 29..21 | 20..12 | 11..0      |
//
// PTE 格式（64-bit）：
//   | RPLV/NX/NR | 保留 | PPN[47:12] | 软件位 | G/MAT/PLV/D/V |
//
// 标志位：
//   bit 0: V（硬件有效）
//   bit 1: D（硬件脏/可写）
//   bits[3:2]: PLV（特权级：0=内核，3=用户）
//   bits[5:4]: MAT（内存类型：1=缓存）
//   bit 6: G（全局映射）
//   bit 7: P（软件 present）
//   bit 8: W（软件记录的可写权限）
//   bit 10: PROTNONE（软件存在、硬件禁止访问的叶子）
//   bit 61: NR（禁止读）
//   bit 62: NX（禁止执行）
//   bit 63: RPLV
//
//! LoongArch 页表不仅编码 Linux VMA 权限，还参与软件 TLB refill、10-bit ASID、huge leaf、
//! Global 配对约束和跨 hart INVTLB。PTE 的 `P/W/PROTNONE` 软件位不能简单等同于硬件
//! `V/D/NR/NX`；mprotect、COW 和 page-mkwrite 需要借助它们区分“逻辑存在但暂不可访问”。
//!
//! 修改叶子后必须按地址空间 residency 完成远端失效，页表页和 data frame 在 completion
//! 之前进入 retired 队列而不是立即释放。软件 refill 会缓存 invalid pair，因此即使从无效
//! PTE 建立 fresh mapping，也不能未经证明删除定点 invalidation。

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
    // PTE 已删除或替换的数据帧。与所属地址空间一起保留到同步 TLB 刷新完成；独立于
    // 进程的延迟回收队列无法为包含多个 ASID 的批次安全选择目标硬件线程集合。
    retired_data_frames: Vec<Arc<FrameTracker>>,
    // 退出任务仍在此根页表上运行时不能复用页表帧；最后一次活动硬件线程转换
    // 会释放这批归属明确的帧。
    retired_page_table_frames: Mutex<Vec<FrameTracker>>,
    // 保守的半开 VPN 包络，覆盖上次同步刷新后每次成功的叶 PTE 修改。稀疏修改之间
    // 可包含未触碰页，但绝不遗漏修改页。`&mut PTE` 写入者使用 get_mut()；activate() 只需
    // 执行内部状态复位。
    pending_tlb_range: Mutex<PendingTlbRange>,
}

impl PageTable {
    /// 首次激活前把已构建且 immutable 的 kernel half 标为 global。LoongArch 4 KiB TLB entry
    /// 覆盖偶/奇页对，只有两个 TLBELO 的 G 位都置位时才真正 global。未配对 leaf 保持
    /// non-global，不能声称硬件不提供的属性。recycled kernel stack 等 runtime mapping 在此
    /// pass 后安装，并刻意保持 ASID-scoped。
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

    /// 在 PMD 中安装一个对齐的 2 MiB kernel direct-map entry。
    ///
    /// 刻意拒绝 Global huge-leaf 编码。旧 bit-12 假设在访问 high-RAM direct map 时立即 fault；
    /// 4 KiB 配对 global leaf 另行独立评估。
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

    /// 用户 root 复制 kernel half 前填充所有仍为空的 kernel-half root entry。后续动态 mapping
    /// 只修改共享 lower-level table，旧用户 root 就不会遗漏新建 kernel-stack root branch。
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

    /// 不分配页表页，原子替换已有 leaf mapping；供 COW 在 replacement frame 完全准备好后使用。
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
        // QEMU 10.0.2 执行 LDPTE 时会屏蔽 NR/NX。保持 leaf software-present 但
        // hardware-invalid，使 PROT_NONE 在该 QEMU、真机和新 emulator 上都不可访问。
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
        // 调用方以 is_valid() 判断 resident/software-present。PROT_NONE leaf 刻意清硬件 V，
        // 但 mprotect()、munmap() 与 fork() 仍必须能找到它。
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
