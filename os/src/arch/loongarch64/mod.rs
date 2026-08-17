//! LoongArch64 架构适配层。
//!
//! 对外接口尽量和 `rv64` 保持一致：入口、trap、timer、task、页表和
//! MMU token 都从这里导出。LoongArch 额外需要 PCI、TLB refill 和若干
//! CSR 封装，因此会比 RISC-V 多出一些启动过渡代码。

pub mod config;
mod entry;
#[cfg(feature = "board_ls2k1000")]
pub mod fdt;
pub mod interrupt;
pub mod mm;
// LoongArch virt 机器上的块设备经 PCI 暴露。
pub mod pci;
// LoongArch CSR 缺少成熟 crate 覆盖，这里保留本地寄存器封装。
pub mod register;
pub mod sbi;
pub mod smp;
pub mod task;
pub mod timer;
pub mod trap;

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

pub use entry::enter_main;

global_asm!(include_str!("tlb_refill.S"));

static LOW_DIRECT_MAP_ACTIVE: AtomicBool = AtomicBool::new(true);

/// 2K1000LA 真机经 U-Boot `go` 进入时 CRMD.PG 已为 1（分页模式，靠 DMW 窗口），
/// `paging_enabled()` 恒真，无法用于判断内核页表是否已安装。boot paging 用此标志
/// 保证幂等（首次进入必建表，mm::init 第二次调用直接返回）。
#[cfg(feature = "board_ls2k1000")]
static BOOT_PAGING_DONE: AtomicBool = AtomicBool::new(false);

const LOONGARCH_CPUCFG1: usize = 1;
const CPUCFG1_UAL: usize = 1 << 20;
const HWCAP_LOONGARCH_UAL: usize = 1 << 2;

/// Return the Linux-compatible ELF hardware capability mask for user space.
///
/// Linux only advertises UAL after observing CPUCFG1.UAL.  In particular,
/// native LoongArch QEMU's TCG backend relies on this bit before using
/// unaligned host loads and stores.
#[inline]
pub fn elf_hwcap() -> usize {
    let config: usize;
    unsafe {
        asm!(
            "cpucfg {config}, {index}",
            config = out(reg) config,
            index = in(reg) LOONGARCH_CPUCFG1,
            options(nomem, nostack)
        );
    }
    if config & CPUCFG1_UAL != 0 {
        HWCAP_LOONGARCH_UAL
    } else {
        0
    }
}

// QEMU 启动时先依赖低地址 DMW 直映运行；进入高地址共享内核模型前，
// 需要一份覆盖早期内核镜像和高端 256 MiB 内核堆的临时页表作为过渡。
const BOOT_MAP_SIZE: usize = 128 * 1024 * 1024;
const BOOT_PTE_TABLES: usize = BOOT_MAP_SIZE / (512 * crate::config::PAGE_SIZE);
const BOOT_HIGH_MAP_SIZE: usize = 512 * 1024 * 1024;
const BOOT_HIGH_PTE_TABLES: usize = BOOT_HIGH_MAP_SIZE / (512 * crate::config::PAGE_SIZE);

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
static mut BOOT_HIGH_PMD: BootPage = BootPage([0; 512]);
static mut BOOT_HIGH_PTES: [BootPage; BOOT_HIGH_PTE_TABLES] =
    [const { BootPage([0; 512]) }; BOOT_HIGH_PTE_TABLES];

unsafe extern "C" {
    fn __rfill();
}

#[inline]
pub fn read_mmu_token() -> usize {
    register::mmu::read_pgdl() | register::mmu::read_asid()
}

#[inline]
pub fn write_mmu_token(token: usize) {
    unsafe {
        // 当前模型下用户低半区和内核高半区共享同一个根页表页。
        // LoongArch 的硬件按虚拟地址所在半区在 PGDL/PGDH 中选择根页表，
        // 因此两个寄存器都要写入当前地址空间的 root。
        let root = token & !0xfff;
        register::mmu::write_pgdl(root);
        register::mmu::write_pgdh(root);
        register::mmu::write_asid(token);
        register::mmu::sync_page_table_root();
    }
}

#[inline]
pub fn sfence() {
    crate::perf::full_tlb_invalidation(1);
    unsafe {
        register::mmu::flush_tlb();
    }
}

#[inline]
pub fn sfence_asid(asid: usize) {
    assert!(asid < 1024, "LoongArch ASID exceeds the 10-bit field");
    crate::perf::asid_tlb_invalidation(1);
    unsafe {
        register::mmu::flush_tlb_asid(asid);
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

#[inline(always)]
pub fn wait_for_interrupt() {
    unsafe {
        core::arch::asm!("idle 0", options(nomem, nostack));
    }
}

#[inline]
fn kernel_virt_to_phys<T>(ptr: *const T) -> usize {
    let addr = ptr as usize;
    if addr >= crate::config::KERNEL_BASE {
        addr - crate::config::KERNEL_BASE
    } else {
        // 低于 KERNEL_BASE 的运行时地址可能是：
        // - QEMU 直接地址模式下的物理地址（< 48-bit，掩码后不变）；
        // - 2K1000LA U-Boot 的 0x9000 cached / 0x8000 uncached 窗口 VA（VSEG 在高
        //   16 位，低 48 位即物理地址）。统一掩到 48-bit 得到真实物理地址。
        addr & ((1usize << 48) - 1)
    }
}

#[inline]
pub unsafe fn jump_to_high_half(entry: usize) -> ! {
    // entry 是函数地址（PC 相对寻址，运行时落在当前窗口）：
    // - QEMU 直接地址模式下是物理地址，+ KERNEL_BASE 得到高半区；
    // - 2K1000LA 是 0x9000 窗口 VA，需先掩掉高 16 位窗口前缀得到物理地址再加 KERNEL_BASE。
    let target = if entry >= crate::config::KERNEL_BASE {
        entry
    } else {
        (entry & ((1usize << 48) - 1)) + crate::config::KERNEL_BASE
    };
    unsafe {
        core::arch::asm!(
            "bgeu    $sp, {kernel_base}, 1f",
            // 把 $sp 归一化到物理地址再重定位到高半区：
            // - QEMU：$sp 已是物理地址（< 48-bit），slli/srli 为 no-op；
            // - 2K1000LA：$sp 在 0x9000/0x8000 DMW 窗口（VSEG 高 16 位），
            //   slli/srli 清掉高 16 位得到物理地址，再加 KERNEL_BASE。
            "slli.d  $sp, $sp, 16",
            "srli.d  $sp, $sp, 16",
            "add.d   $sp, $sp, {kernel_base}",
            "1:",
            "jr      {target}",
            kernel_base = in(reg) crate::config::KERNEL_BASE,
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
    #[cfg(feature = "board_ls2k1000")]
    if BOOT_PAGING_DONE.swap(true, Ordering::AcqRel) {
        // 2K1000LA 真机经 U-Boot `go` 进入时 CRMD.PG 已为 1，paging_enabled() 恒真，
        // 不能作为“内核页表是否已安装”的判断，改由 BOOT_PAGING_DONE 保证幂等。
        return;
    }
    #[cfg(not(feature = "board_ls2k1000"))]
    if paging_enabled() {
        return;
    }
    unsafe {
        // 写指针用 addr_of! 直接得到的「当前可达地址」：
        // - QEMU 直接地址模式（DA=1）下，PC 相对寻址落在物理地址，可直接解引用；
        // - 2K1000LA 经 U-Boot go 进入时，PC 相对寻址自动落在 0x9000 cached 窗口 VA。
        // 若改用 kernel_virt_to_phys 得到的 48-bit 物理地址，真机上 VSEG=0x0000 无
        // DMW/页表覆盖，写页表会页错误。
        // PTE 里存的 PPN / CSR 根页表地址必须用 kernel_virt_to_phys 得到的物理地址。
        let pgd = core::ptr::addr_of!(BOOT_PGD.0) as *mut [usize; 512];
        let pmd = core::ptr::addr_of!(BOOT_PMD.0) as *mut [usize; 512];
        let ptes = core::ptr::addr_of!(BOOT_PTES) as *mut [BootPage; BOOT_PTE_TABLES];
        let ptes_phys = kernel_virt_to_phys(core::ptr::addr_of!(BOOT_PTES) as *const _);
        let high_pmd = core::ptr::addr_of!(BOOT_HIGH_PMD.0) as *mut [usize; 512];
        let high_ptes =
            core::ptr::addr_of!(BOOT_HIGH_PTES) as *mut [BootPage; BOOT_HIGH_PTE_TABLES];
        let high_ptes_phys =
            kernel_virt_to_phys(core::ptr::addr_of!(BOOT_HIGH_PTES) as *const _);

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
            let table_ptr = ptes as usize + table * core::mem::size_of::<BootPage>();
            let table_phys = ptes_phys + table * core::mem::size_of::<BootPage>();
            core::ptr::write_volatile(
                (pmd as *mut usize).add(pmd_idx + table),
                table_pte(table_phys),
            );
            for idx in 0..512 {
                let pa = (table * 512 + idx) * crate::config::PAGE_SIZE;
                core::ptr::write_volatile((table_ptr as *mut usize).add(idx), leaf_pte(pa));
            }
        }
        let high_va = crate::config::KERNEL_BASE + crate::config::HIGH_MEMORY_START;
        let high_vpn = high_va >> crate::config::PAGE_SIZE_BITS;
        let high_pgd_idx = (high_vpn >> 18) & 0x1ff;
        let high_pmd_idx = (high_vpn >> 9) & 0x1ff;
        core::ptr::write_volatile(
            (pgd as *mut usize).add(high_pgd_idx),
            table_pte(kernel_virt_to_phys(
                core::ptr::addr_of!(BOOT_HIGH_PMD) as *const _
            )),
        );
        for table in 0..BOOT_HIGH_PTE_TABLES {
            let table_ptr = high_ptes as usize + table * core::mem::size_of::<BootPage>();
            let table_phys = high_ptes_phys + table * core::mem::size_of::<BootPage>();
            core::ptr::write_volatile(
                (high_pmd as *mut usize).add(high_pmd_idx + table),
                table_pte(table_phys),
            );
            for idx in 0..512 {
                let pa = crate::config::HIGH_MEMORY_START
                    + (table * 512 + idx) * crate::config::PAGE_SIZE;
                core::ptr::write_volatile((table_ptr as *mut usize).add(idx), leaf_pte(pa));
            }
        }
        configure_mmu();
        let root = kernel_virt_to_phys(core::ptr::addr_of!(BOOT_PGD) as *const _);
        write_mmu_token(root);

        // 2K1000LA 经 U-Boot 进入时 PG 已为 1，U-Boot 的 TLB 里可能残留旧翻译；
        // 安装内核根页表后先冲刷 TLB，再保证分页与 MAT 配置正确。
        #[cfg(feature = "board_ls2k1000")]
        register::mmu::flush_tlb();
        register::crmd::enable_paging();
        // QEMU：开启分页后立即关 DMW1，低地址直映由 DMW0 兜底，代码继续跑在物理
        // 地址直到 jump_to_high_half。2K1000LA 相反：当前执行流在 0x9000 cached 窗口，
        // 必须保留 DMW1 直到跳入高半区（随后由 disable_low_direct_map 关闭）。
        #[cfg(not(feature = "board_ls2k1000"))]
        register::mmu::write_dmw1(0);
    }
}

/// Secondary harts enter through QEMU's physical mailbox after the boot hart
/// has finished building `BOOT_PGD`.  They must install the existing boot
/// root locally, but must not rebuild the shared tables concurrently.
pub fn enable_secondary_boot_paging() {
    if paging_enabled() {
        return;
    }
    unsafe {
        configure_mmu();
        let root = kernel_virt_to_phys(core::ptr::addr_of!(BOOT_PGD) as *const _);
        write_mmu_token(root);
        register::crmd::enable_paging();
        // 与 enable_boot_paging 相同的 DMW 策略；2K1000LA 次核同样运行在 0x9000
        // 窗口，跳入高半区前不能关 DMW1（SMP 在 Stage 7，此路径尚未真机验证）。
        #[cfg(not(feature = "board_ls2k1000"))]
        register::mmu::write_dmw1(0);
    }
}

/// 开启 MMU：正式页表激活前若还未分页，则先走 boot page table 过渡。
pub fn enable_mmu() {
    enable_boot_paging();
}

pub fn disable_low_direct_map() {
    unsafe {
        // QEMU：低地址直映走 DMW0，关闭它让后续访问全部走页表。
        // 2K1000LA：低地址直映走 DMW1（0x9000 cached 窗口）；DMW0（0x8000 uncached）
        // 是真机 MMIO 访问窗口，必须保留，否则 UART/外设写不达（见 keypoints 坑 2）。
        #[cfg(feature = "board_ls2k1000")]
        register::mmu::write_dmw1(0);
        #[cfg(not(feature = "board_ls2k1000"))]
        register::mmu::write_dmw0(0);
        register::mmu::flush_tlb();
    }
    LOW_DIRECT_MAP_ACTIVE.store(false, Ordering::Relaxed);
}
