// os/src/arch/loongarch64/mod.rs
// LoongArch 64 架构模块

pub mod config;
mod entry;
pub mod interrupt;
pub mod mm;
pub mod register;
pub mod sbi;
pub mod task;
pub mod timer;
pub mod trap;

pub use entry::enter_main;

#[inline]
pub fn read_mmu_token() -> usize {
    register::mmu::read_pgdh()
}

#[inline]
pub fn write_mmu_token(token: usize) {
    unsafe {
        register::mmu::write_pgdh(token);
    }
}

#[inline]
pub fn sfence() {
    unsafe {
        register::mmu::flush_tlb();
    }
}

/// 开启 MMU：配置 DMW0 为内核恒等映射，然后启动分页
///
/// DMW0: VA[47:44]=0 → PA[47:44]=0，缓存模式，仅 PLV0（内核）可用。
/// 内核访问低 256GB 地址空间时走 DMW 直接映射，不经过页表。
/// 用户态（PLV3）访问不命中 DMW，必须走页表。
///
/// CRMD: 清 DA（关闭直接地址模式），置 PG（开启分页）。
pub fn enable_mmu() {
    unsafe {
        // DMW0: VSEG=0, PSEG=0, MAT=1 (cached), PLV0=1, PLV3=0
        // DMW0 bit layout: [47:44]=VSEG, [43:40]=PSEG, [5:4]=MAT, [3]=PLV3, [2]=PLV0
        let dmw0: usize = (1 << 4) | (1 << 2); // MAT=1(cached), PLV0=1
        register::mmu::write_dmw0(dmw0);

        // DMW1: 清零，暂不配置
        register::mmu::write_dmw1(0);

        // CRMD: 清 DA(bit3), 置 PG(bit4)
        // DA=1 为直接地址模式，PG=1 为分页模式
        register::crmd::enable_paging();

        // 从 DA=1 切换到 PG=1 后，TLB 应当为空（DA=1 时 bypass TLB），
        // 但为安全起见做一次全刷新
        register::mmu::flush_tlb();
    }
}
