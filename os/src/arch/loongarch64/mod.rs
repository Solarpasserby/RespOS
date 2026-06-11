//! LoongArch64 架构适配层。
//!
//! 对外接口尽量和 `rv64` 保持一致：入口、trap、timer、task、页表和
//! MMU token 都从这里导出。LoongArch 额外需要 PCI、TLB refill 和若干
//! CSR 封装，因此会比 RISC-V 多出一些启动过渡代码。

pub mod config;
mod entry;
pub mod interrupt;
pub mod mm;
// LoongArch virt 机器上的块设备经 PCI 暴露。
pub mod pci;
// LoongArch CSR 缺少成熟 crate 覆盖，这里保留本地寄存器封装。
pub mod register;
pub mod sbi;
pub mod task;
pub mod timer;
pub mod trap;

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, Ordering};

pub use entry::enter_main;

global_asm!(include_str!("tlb_refill.S"));

static LOW_DIRECT_MAP_ACTIVE: AtomicBool = AtomicBool::new(true);

// QEMU 启动时先依赖低地址 DMW 直映运行；进入高地址共享内核模型前，
// 需要一份只覆盖早期内核镜像和启动堆的临时页表作为过渡。
const BOOT_MAP_SIZE: usize = 64 * 1024 * 1024;
const BOOT_PTE_TABLES: usize = BOOT_MAP_SIZE / (512 * crate::config::PAGE_SIZE);

const PTE_VALID: usize = 1 << 0;
const PTE_DIRTY: usize = 1 << 1;
const PTE_MAT_CC: usize = 1 << 4;
const PTE_GLOBAL: usize = 1 << 6;
const PTE_PRESENT: usize = 1 << 7;
const PTE_WRITABLE: usize = 1 << 8;

#[repr(align(4096))]
struct BootPage([usize; 512]);

static mut BOOT_PGD: BootPage = BootPage([0; 512]);
static mut BOOT_PMD: BootPage = BootPage([0; 512]);
static mut BOOT_PTES: [BootPage; BOOT_PTE_TABLES] = [const { BootPage([0; 512]) }; BOOT_PTE_TABLES];

unsafe extern "C" {
    fn __rfill();
}

#[inline]
pub fn read_mmu_token() -> usize {
    register::mmu::read_pgdl()
}

#[inline]
pub fn write_mmu_token(token: usize) {
    unsafe {
        // 当前模型下用户低半区和内核高半区共享同一个根页表页。
        // LoongArch 的硬件按虚拟地址所在半区在 PGDL/PGDH 中选择根页表，
        // 因此两个寄存器都要写入当前地址空间的 root。
        register::mmu::write_pgdl(token);
        register::mmu::write_pgdh(token);
        register::mmu::write_asid(0);
        register::mmu::sync_page_table_root();
    }
}

#[inline]
pub fn sfence() {
    unsafe {
        register::mmu::flush_tlb();
    }
}

#[inline]
pub fn paging_enabled() -> bool {
    register::crmd::paging_enabled()
}

#[inline]
pub fn low_direct_map_enabled() -> bool {
    LOW_DIRECT_MAP_ACTIVE.load(Ordering::Relaxed)
}

#[inline]
pub fn enable_kernel_extensions() {
    unsafe {
        register::euen::enable_kernel_extensions();
    }
}

#[inline(always)]
pub fn idle() -> ! {
    register::idle()
}

#[inline]
fn kernel_virt_to_phys<T>(ptr: *const T) -> usize {
    let addr = ptr as usize;
    if addr >= crate::config::KERNEL_BASE {
        addr - crate::config::KERNEL_BASE
    } else {
        addr
    }
}

#[inline]
pub unsafe fn jump_to_high_half(entry: usize) -> ! {
    let target = if entry >= crate::config::KERNEL_BASE {
        entry
    } else {
        entry + crate::config::KERNEL_BASE
    };
    unsafe {
        core::arch::asm!(
            "li.d    $t0, {kernel_base}",
            "bgeu    $sp, $t0, 1f",
            "add.d   $sp, $sp, $t0",
            "1:",
            "jr      {target}",
            kernel_base = const crate::config::KERNEL_BASE,
            target = in(reg) target,
            options(noreturn)
        );
    }
}

#[inline]
fn table_pte(pa: usize) -> usize {
    ((pa >> crate::config::PAGE_SIZE_BITS) << 12) | PTE_VALID
}

#[inline]
fn leaf_pte(pa: usize) -> usize {
    ((pa >> crate::config::PAGE_SIZE_BITS) << 12)
        | PTE_VALID
        | PTE_DIRTY
        | PTE_MAT_CC
        | PTE_GLOBAL
        | PTE_PRESENT
        | PTE_WRITABLE
}

unsafe fn configure_mmu() {
    let refill_entry_pa = kernel_virt_to_phys(__rfill as *const ());
    unsafe {
        register::mmu::write_tlbrentry(refill_entry_pa);
        register::mmu::write_asid(0);
        register::mmu::configure_tlb_page_size();
        register::mmu::configure_page_walk();
    }
}

/// 建立一个最小的高地址恒等偏移映射，使高地址内核堆在正式内核页表构造前可用。
pub fn enable_boot_paging() {
    if paging_enabled() {
        return;
    }
    unsafe {
        let pgd =
            kernel_virt_to_phys(core::ptr::addr_of!(BOOT_PGD.0) as *const _) as *mut [usize; 512];
        let pmd =
            kernel_virt_to_phys(core::ptr::addr_of!(BOOT_PMD.0) as *const _) as *mut [usize; 512];
        let ptes = kernel_virt_to_phys(core::ptr::addr_of!(BOOT_PTES) as *const _)
            as *mut [BootPage; BOOT_PTE_TABLES];

        let base_vpn = crate::config::KERNEL_BASE >> crate::config::PAGE_SIZE_BITS;
        let pgd_idx = (base_vpn >> 18) & 0x1ff;
        let pmd_idx = (base_vpn >> 9) & 0x1ff;

        core::ptr::write_volatile(
            (pgd as *mut usize).add(pgd_idx),
            table_pte(kernel_virt_to_phys(
                core::ptr::addr_of!(BOOT_PMD) as *const _
            )),
        );
        for table in 0..BOOT_PTE_TABLES {
            let table_pa = ptes as usize + table * core::mem::size_of::<BootPage>();
            core::ptr::write_volatile(
                (pmd as *mut usize).add(pmd_idx + table),
                table_pte(table_pa),
            );
            for idx in 0..512 {
                let pa = (table * 512 + idx) * crate::config::PAGE_SIZE;
                core::ptr::write_volatile((table_pa as *mut usize).add(idx), leaf_pte(pa));
            }
        }
        configure_mmu();
        let root = kernel_virt_to_phys(core::ptr::addr_of!(BOOT_PGD) as *const _);
        write_mmu_token(root);

        register::crmd::enable_paging();
        register::mmu::write_dmw1(0);
    }
}

/// 开启 MMU：正式页表激活前若还未分页，则先走 boot page table 过渡。
pub fn enable_mmu() {
    enable_boot_paging();
}

pub fn disable_low_direct_map() {
    unsafe {
        register::mmu::write_dmw0(0);
        register::mmu::flush_tlb();
    }
    LOW_DIRECT_MAP_ACTIVE.store(false, Ordering::Relaxed);
}
