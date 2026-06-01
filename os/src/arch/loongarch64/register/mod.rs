//! Minimal LoongArch64 CSR helpers used by this kernel.
//!
//! Keep this module deliberately small. It mirrors only the `riscv::register`
//! surface that the current kernel actually relies on.

macro_rules! read_csr {
    ($csr:expr) => {{
        let bits: usize;
        unsafe {
            core::arch::asm!("csrrd {}, {}", out(reg) bits, const $csr, options(nomem, nostack));
        }
        bits
    }};
}

macro_rules! write_csr {
    ($csr:expr, $bits:expr) => {{
        unsafe {
            core::arch::asm!("csrwr {}, {}", in(reg) $bits, const $csr, options(nomem, nostack));
        }
    }};
}

pub mod crmd {
    const CSR_CRMD: usize = 0x0;
    const IE: usize = 1 << 2;
    const DA: usize = 1 << 3;
    const PG: usize = 1 << 4;
    const DATF_SHIFT: usize = 5;
    const DATM_SHIFT: usize = 7;
    const MAT_CC: usize = 0b01;

    #[inline(always)]
    pub fn read() -> usize {
        read_csr!(CSR_CRMD)
    }

    #[inline(always)]
    pub unsafe fn write(bits: usize) {
        write_csr!(CSR_CRMD, bits);
    }

    #[inline(always)]
    pub fn interrupt_enabled() -> bool {
        read() & IE != 0
    }

    #[inline(always)]
    pub unsafe fn set_interrupt_enabled(enabled: bool) {
        let mut bits = read();
        if enabled {
            bits |= IE;
        } else {
            bits &= !IE;
        }
        unsafe {
            write(bits);
        }
    }

    #[inline(always)]
    pub unsafe fn enable_paging() {
        let mut bits = read();
        bits &= !DA;
        bits |= PG;
        bits &= !(0b11 << DATF_SHIFT);
        bits &= !(0b11 << DATM_SHIFT);
        bits |= MAT_CC << DATF_SHIFT;
        bits |= MAT_CC << DATM_SHIFT;
        unsafe {
            write(bits);
        }
    }
}

pub mod ecfg {
    const CSR_ECFG: usize = 0x4;
    const TIMER_INTERRUPT: usize = 1 << 11;

    #[inline(always)]
    pub fn read() -> usize {
        read_csr!(CSR_ECFG)
    }

    #[inline(always)]
    pub unsafe fn write(bits: usize) {
        write_csr!(CSR_ECFG, bits);
    }

    #[inline(always)]
    pub unsafe fn enable_timer_interrupt() {
        unsafe {
            write(read() | TIMER_INTERRUPT);
        }
    }
}

pub mod euen {
    const CSR_EUEN: usize = 0x2;
    const FPE: usize = 1 << 0;
    const SXE: usize = 1 << 1;

    #[inline(always)]
    pub fn read() -> usize {
        read_csr!(CSR_EUEN)
    }

    #[inline(always)]
    pub unsafe fn write(bits: usize) {
        write_csr!(CSR_EUEN, bits);
    }

    #[inline(always)]
    pub unsafe fn enable_kernel_extensions() {
        unsafe {
            write(read() | FPE | SXE);
        }
    }
}

pub mod estat {
    const CSR_ESTAT: usize = 0x5;
    const IS_MASK: usize = (1 << 13) - 1;
    const ECODE_SHIFT: usize = 16;
    const ECODE_MASK: usize = 0x3f;

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum Exception {
        PageInvalidLoad,
        PageInvalidStore,
        PageInvalidFetch,
        PageModifyFault,
        PageNonReadable,
        PageNonExecutable,
        PagePrivilegeIllegal,
        AddressError,
        AddressNotAligned,
        BoundsCheck,
        Syscall,
        Breakpoint,
        IllegalInstruction,
        PrivilegedInstruction,
        FloatingPointUnavailable,
        SimdUnavailable,
        Unknown(usize),
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum Interrupt {
        Software0,
        Software1,
        Hardware(usize),
        PerformanceCounter,
        Timer,
        Ipi,
        Unknown(usize),
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum Trap {
        Exception(Exception),
        Interrupt(Interrupt),
    }

    #[inline(always)]
    pub fn read() -> usize {
        read_csr!(CSR_ESTAT)
    }

    #[inline(always)]
    pub fn interrupt_bits(bits: usize) -> usize {
        bits & IS_MASK
    }

    #[inline(always)]
    pub fn ecode(bits: usize) -> usize {
        (bits >> ECODE_SHIFT) & ECODE_MASK
    }

    pub fn cause(bits: usize) -> Trap {
        let is = interrupt_bits(bits);
        if is != 0 {
            return Trap::Interrupt(interrupt_from_index(is.trailing_zeros() as usize));
        }
        Trap::Exception(exception_from_ecode(ecode(bits)))
    }

    fn exception_from_ecode(ecode: usize) -> Exception {
        match ecode {
            0x1 => Exception::PageInvalidLoad,
            0x2 => Exception::PageInvalidStore,
            0x3 => Exception::PageInvalidFetch,
            0x4 => Exception::PageModifyFault,
            0x5 => Exception::PageNonReadable,
            0x6 => Exception::PageNonExecutable,
            0x7 => Exception::PagePrivilegeIllegal,
            0x8 => Exception::AddressError,
            0x9 => Exception::AddressNotAligned,
            0xa => Exception::BoundsCheck,
            0xb => Exception::Syscall,
            0xc => Exception::Breakpoint,
            0xd => Exception::IllegalInstruction,
            0xe => Exception::PrivilegedInstruction,
            0xf => Exception::FloatingPointUnavailable,
            0x10 => Exception::SimdUnavailable,
            other => Exception::Unknown(other),
        }
    }

    fn interrupt_from_index(index: usize) -> Interrupt {
        match index {
            0 => Interrupt::Software0,
            1 => Interrupt::Software1,
            2..=9 => Interrupt::Hardware(index - 2),
            10 => Interrupt::PerformanceCounter,
            11 => Interrupt::Timer,
            12 => Interrupt::Ipi,
            other => Interrupt::Unknown(other),
        }
    }
}

pub mod eentry {
    const CSR_EENTRY: usize = 0xc;

    #[inline(always)]
    pub unsafe fn write(addr: usize) {
        write_csr!(CSR_EENTRY, addr);
    }
}

pub mod era {
    const CSR_ERA: usize = 0x6;

    #[inline(always)]
    pub fn read() -> usize {
        read_csr!(CSR_ERA)
    }
}

pub mod badv {
    const CSR_BADV: usize = 0x7;

    #[inline(always)]
    pub fn read() -> usize {
        read_csr!(CSR_BADV)
    }
}

pub mod timer {
    const CSR_TCFG: usize = 0x41;
    const CSR_TICLR: usize = 0x44;
    const TCFG_ENABLE: usize = 1 << 0;
    const TCFG_PERIODIC: usize = 1 << 1;
    const TICLR_CLR: usize = 1 << 0;

    #[inline(always)]
    pub fn read_time() -> usize {
        let low: usize;
        let high: usize;
        unsafe {
            core::arch::asm!(
                "rdtime.d {}, {}",
                out(reg) low,
                out(reg) high,
                options(nomem, nostack)
            );
        }
        (high << 32) | (low & 0xffff_ffff)
    }

    #[inline(always)]
    pub unsafe fn set_oneshot(ticks: usize) {
        let ticks = ticks.max(4);
        write_csr!(CSR_TCFG, TCFG_ENABLE | (ticks & !0b11));
    }

    #[inline(always)]
    pub unsafe fn set_periodic(ticks: usize) {
        let ticks = ticks.max(4);
        write_csr!(CSR_TCFG, TCFG_ENABLE | TCFG_PERIODIC | (ticks & !0b11));
    }

    #[inline(always)]
    pub unsafe fn clear_interrupt() {
        write_csr!(CSR_TICLR, TICLR_CLR);
    }
}

pub mod mmu {
    use crate::config::PAGE_SIZE_BITS;

    const CSR_PGDL: usize = 0x19;
    const CSR_PGDH: usize = 0x1a;
    const CSR_PWCL: usize = 0x1c;
    const CSR_PWCH: usize = 0x1d;
    const CSR_STLBPS: usize = 0x1e;
    const CSR_TLBRENTRY: usize = 0x88;
    const CSR_TLBREHI: usize = 0x8e;
    const CSR_DMW0: usize = 0x180;
    const CSR_DMW1: usize = 0x181;

    #[inline(always)]
    pub fn read_pgdl() -> usize {
        read_csr!(CSR_PGDL)
    }

    #[inline(always)]
    pub fn read_pgdh() -> usize {
        read_csr!(CSR_PGDH)
    }

    #[inline(always)]
    pub unsafe fn write_pgdl(bits: usize) {
        write_csr!(CSR_PGDL, bits);
    }

    #[inline(always)]
    pub unsafe fn write_pgdh(bits: usize) {
        write_csr!(CSR_PGDH, bits);
    }

    #[inline(always)]
    pub unsafe fn write_tlbrentry(bits: usize) {
        write_csr!(CSR_TLBRENTRY, bits);
    }

    #[inline(always)]
    pub unsafe fn configure_page_walk() {
        const DIR_WIDTH: usize = 9;
        let pwcl = PAGE_SIZE_BITS
            | (DIR_WIDTH << 5)
            | ((PAGE_SIZE_BITS + DIR_WIDTH) << 10)
            | (DIR_WIDTH << 15)
            | ((PAGE_SIZE_BITS + DIR_WIDTH * 2) << 20)
            | (DIR_WIDTH << 25);
        let pwch = ((PAGE_SIZE_BITS + DIR_WIDTH * 3) << 0) | (DIR_WIDTH << 6);
        write_csr!(CSR_PWCL, pwcl);
        write_csr!(CSR_PWCH, pwch);
    }

    #[inline(always)]
    pub unsafe fn configure_tlb_page_size() {
        write_csr!(CSR_STLBPS, PAGE_SIZE_BITS);
        write_csr!(CSR_TLBREHI, PAGE_SIZE_BITS);
    }

    #[inline(always)]
    pub unsafe fn write_dmw0(bits: usize) {
        write_csr!(CSR_DMW0, bits);
    }

    #[inline(always)]
    pub unsafe fn write_dmw1(bits: usize) {
        write_csr!(CSR_DMW1, bits);
    }

    #[inline(always)]
    pub unsafe fn flush_tlb() {
        unsafe {
            core::arch::asm!("invtlb 0x3, $r0, $r0", options(nostack));
        }
    }
}

#[inline(always)]
pub fn idle() -> ! {
    loop {
        unsafe {
            core::arch::asm!("idle 0", options(nomem, nostack));
        }
    }
}
