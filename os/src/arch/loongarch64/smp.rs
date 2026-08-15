//! LoongArch QEMU-virt SMP startup and per-CPU interrupt plumbing.
//!
//! QEMU's auxiliary boot ROM parks every non-boot CPU in `idle`, waiting for
//! mailbox 0 and an IPI.  The boot CPU publishes the physical secondary entry
//! through `IOCSR_MBUF_SEND`, then sends vector 0 through `IOCSR_IPI_SEND`.

use core::{
    arch::asm,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering, fence},
};

pub const MAX_HARTS: usize = 12;

const BOOT_COLD: u8 = 0;
const BOOT_READY: u8 = 1;
const BOOT_RELEASED: u8 = 2;
const IOCSR_IPI_STATUS: usize = 0x1000;
const IOCSR_IPI_EN: usize = 0x1004;
const IOCSR_IPI_CLEAR: usize = 0x100c;
const IOCSR_IPI_SEND: usize = 0x1040;
const IOCSR_MBUF_SEND: usize = 0x1048;
const SEND_BLOCKING: usize = 1 << 31;
const SEND_CPU_SHIFT: usize = 16;
const MBUF_BOX_SHIFT: usize = 2;
const MBUF_BUF_SHIFT: usize = 32;
const IPI_SCHEDULER: usize = 0;
const IPI_TLB_SHOOTDOWN: usize = 1;

static BOOT_STATE: AtomicU8 = AtomicU8::new(BOOT_COLD);
static ONLINE_HART_MASK: AtomicUsize = AtomicUsize::new(0);
static IDLE_HART_MASK: AtomicUsize = AtomicUsize::new(0);
static IPI_COUNT: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];
static TLB_SHOOTDOWN_GENERATION: AtomicUsize = AtomicUsize::new(1);
static TLB_SHOOTDOWN_REQUEST: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];
static TLB_SHOOTDOWN_ACK: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];
static TLB_SHOOTDOWN_KIND: [AtomicU8; MAX_HARTS] = [const { AtomicU8::new(0) }; MAX_HARTS];
static TLB_SHOOTDOWN_ASID: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];
static TLB_SHOOTDOWN_START: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];
static TLB_SHOOTDOWN_END: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];

const TLB_KIND_ALL: u8 = 1;
const TLB_KIND_ADDRESS_SPACE: u8 = 2;
const TLB_KIND_RANGE: u8 = 3;

#[derive(Clone, Copy, Debug)]
pub struct TlbShootdownRequest {
    kind: u8,
    asid: usize,
    start: usize,
    end: usize,
}

impl TlbShootdownRequest {
    pub const fn all() -> Self {
        Self {
            kind: TLB_KIND_ALL,
            asid: 0,
            start: 0,
            end: 0,
        }
    }

    pub const fn address_space(asid: usize) -> Self {
        Self {
            kind: TLB_KIND_ADDRESS_SPACE,
            asid,
            start: 0,
            end: 0,
        }
    }

    pub const fn range(asid: usize, start: usize, end: usize) -> Self {
        Self {
            kind: TLB_KIND_RANGE,
            asid,
            start,
            end,
        }
    }

    fn is_valid(self) -> bool {
        match self.kind {
            TLB_KIND_ALL => self.asid == 0 && self.start == 0 && self.end == 0,
            TLB_KIND_ADDRESS_SPACE => self.asid < 1024 && self.start == 0 && self.end == 0,
            TLB_KIND_RANGE => {
                self.asid < 1024
                    && self.start < self.end
                    && self.start % crate::config::PAGE_SIZE == 0
                    && self.end % crate::config::PAGE_SIZE == 0
            }
            _ => false,
        }
    }

    fn record(self) {
        if !self.is_valid() {
            crate::perf::tlb_shootdown_invalid_request(1);
        } else if self.kind == TLB_KIND_ALL {
            crate::perf::tlb_shootdown_all_request(1);
        } else if self.kind == TLB_KIND_ADDRESS_SPACE {
            crate::perf::tlb_shootdown_address_space_request(1);
        } else {
            crate::perf::tlb_shootdown_range_request(1);
            let pages = (self.end - self.start) / crate::config::PAGE_SIZE;
            crate::perf::tlb_shootdown_range_pages(pages);
            crate::perf::observe_tlb_shootdown_range_pages(pages);
            crate::perf::classify_tlb_shootdown_range(pages);
        }
    }

    fn invalidate_local(self) {
        if !self.is_valid() {
            crate::arch::sfence();
            return;
        }
        match self.kind {
            TLB_KIND_ADDRESS_SPACE => crate::arch::sfence_asid(self.asid),
            // A single op=4 safely covers every page in the ASID. Keep range
            // metadata for measurement, but do not rely on op=5 until the
            // full BuildStorm corruption is understood.
            TLB_KIND_RANGE => crate::arch::sfence_asid(self.asid),
            TLB_KIND_ALL => crate::arch::sfence(),
            _ => unreachable!(),
        }
    }
}

unsafe extern "C" {
    fn _start_secondary_phys();
}

#[inline]
fn iocsr_write32(value: usize, address: usize) {
    unsafe {
        asm!(
            "iocsrwr.w {value}, {address}",
            value = in(reg) value,
            address = in(reg) address,
            options(nostack)
        );
    }
}

#[inline]
fn iocsr_write64(value: usize, address: usize) {
    unsafe {
        asm!(
            "iocsrwr.d {value}, {address}",
            value = in(reg) value,
            address = in(reg) address,
            options(nostack)
        );
    }
}

#[inline]
fn iocsr_read32(address: usize) -> usize {
    let value: usize;
    unsafe {
        asm!(
            "iocsrrd.w {value}, {address}",
            value = out(reg) value,
            address = in(reg) address,
            options(nomem, nostack)
        );
    }
    value
}

#[inline]
pub fn current_hart_id() -> usize {
    let id: usize;
    unsafe {
        asm!("csrrd {id}, 0x20", id = out(reg) id, options(nomem, nostack));
    }
    id & 0x3ff
}

#[inline]
pub fn online_hart_mask() -> usize {
    ONLINE_HART_MASK.load(Ordering::Acquire)
}

#[inline]
pub fn is_timer_service_hart() -> bool {
    current_hart_id() == 0
}

fn enable_local_ipi() {
    iocsr_write32(usize::MAX, IOCSR_IPI_CLEAR);
    iocsr_write32(usize::MAX, IOCSR_IPI_EN);
    unsafe {
        crate::arch::register::ecfg::enable_ipi_interrupt();
    }
}

fn send_ipi(hart: usize, vector: usize) {
    let value = SEND_BLOCKING | (hart << SEND_CPU_SHIFT) | vector;
    iocsr_write32(value, IOCSR_IPI_SEND);
}

#[inline]
fn publish_before_iocsr() {
    fence(Ordering::SeqCst);
    unsafe {
        asm!("dbar 0", options(nostack));
    }
}

fn send_mailbox_entry(hart: usize, entry: usize) {
    let common = SEND_BLOCKING | (hart << SEND_CPU_SHIFT);
    let high = common | (1 << MBUF_BOX_SHIFT) | (entry & 0xffff_ffff_0000_0000);
    let low = common | ((entry & 0xffff_ffff) << MBUF_BUF_SHIFT);
    iocsr_write64(high, IOCSR_MBUF_SEND);
    iocsr_write64(low, IOCSR_MBUF_SEND);
}

pub fn publish_boot_ready() {
    enable_local_ipi();
    ONLINE_HART_MASK.store(1, Ordering::Release);
    BOOT_STATE.store(BOOT_READY, Ordering::Release);
}

pub fn start_secondary_harts() {
    let linked = _start_secondary_phys as usize;
    let entry = linked.saturating_sub(crate::config::KERNEL_BASE);
    for hart in 1..MAX_HARTS {
        send_mailbox_entry(hart, entry);
        send_ipi(hart, IPI_SCHEDULER);
    }

    // Give configured secondaries a bounded window to publish themselves so
    // the first userspace sched_getaffinity()/procfs read sees a stable mask.
    // A smaller `-smp` override must still boot, hence the timeout rather than
    // an unconditional wait for all MAX_HARTS bits.
    let expected = (1usize << MAX_HARTS) - 1;
    let started = crate::arch::timer::get_time();
    let timeout = crate::arch::timer::get_hardware_clock_freq();
    while online_hart_mask() != expected
        && crate::arch::timer::get_time().wrapping_sub(started) < timeout
    {
        core::hint::spin_loop();
    }
    println!("[smp] LA online mask={:#x}", online_hart_mask());
}

pub fn release_secondary_harts() {
    BOOT_STATE.store(BOOT_RELEASED, Ordering::Release);
}

pub fn secondary_online() {
    let hart = current_hart_id();
    if hart == 0 || hart >= MAX_HARTS {
        crate::arch::idle();
    }
    while BOOT_STATE.load(Ordering::Acquire) != BOOT_READY {
        core::hint::spin_loop();
    }
    enable_local_ipi();
    ONLINE_HART_MASK.fetch_or(1 << hart, Ordering::AcqRel);
    while BOOT_STATE.load(Ordering::Acquire) != BOOT_RELEASED {
        core::hint::spin_loop();
    }
}

#[inline]
pub fn enter_idle() {
    IDLE_HART_MASK.fetch_or(1 << current_hart_id(), Ordering::Release);
}

#[inline]
pub fn leave_idle() {
    IDLE_HART_MASK.fetch_and(!(1 << current_hart_id()), Ordering::Release);
}

pub fn kick_one_idle_hart_in(allowed_harts: usize) {
    let mask = IDLE_HART_MASK.load(Ordering::Acquire) & online_hart_mask() & allowed_harts;
    if mask == 0 {
        return;
    }
    let hart = mask.trailing_zeros() as usize;
    send_ipi(hart, IPI_SCHEDULER);
    crate::perf::scheduler_ipi(1);
}

/// Notify hart 0 that an earlier task deadline was published. Scheduler and
/// timer notifications may share the wakeup vector because the trap handler
/// reads the deadline from atomic state.
pub fn kick_timer_service_hart() {
    if current_hart_id() != 0 {
        send_ipi(0, IPI_SCHEDULER);
    }
}

/// Service a pending IOCSR IPI while normal kernel execution keeps CRMD.IE=0.
///
/// MemorySet lock acquisition uses this hook so a hart waiting behind a page
/// table writer can still acknowledge that writer's synchronous shootdown.
#[inline]
pub fn poll_pending_ipi() {
    if iocsr_read32(IOCSR_IPI_STATUS) != 0 {
        acknowledge_ipi();
    }
}

/// Flush all local TLB entries on `targets` and wait for every remote hart.
///
/// Each target owns an independent request slot. Requesters acquire slots in
/// ascending hart order, so concurrent shootdowns cannot form an acquisition
/// cycle. One outstanding request per target also makes IOCSR vector
/// coalescing harmless: the target acknowledges the published generation
/// before its slot is reused.
pub fn remote_tlb_shootdown(targets: usize, request: TlbShootdownRequest) {
    request.record();
    debug_assert!(request.is_valid(), "invalid TLB shootdown request");
    let current = current_hart_id();
    let targets = targets & online_hart_mask() & !(1 << current);
    if targets == 0 {
        return;
    }

    let mut generation = TLB_SHOOTDOWN_GENERATION.fetch_add(1, Ordering::Relaxed);
    if generation == 0 {
        generation = TLB_SHOOTDOWN_GENERATION.fetch_add(1, Ordering::Relaxed);
    }

    let mut pending = targets;
    while pending != 0 {
        let hart = pending.trailing_zeros() as usize;
        while TLB_SHOOTDOWN_REQUEST[hart]
            .compare_exchange(0, generation, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            poll_pending_ipi();
            core::hint::spin_loop();
        }
        // Do not let a wrapped generation match an acknowledgement left by a
        // much older request. The slot is exclusively ours at this point.
        TLB_SHOOTDOWN_ACK[hart].store(0, Ordering::Relaxed);
        TLB_SHOOTDOWN_KIND[hart].store(request.kind, Ordering::Relaxed);
        TLB_SHOOTDOWN_ASID[hart].store(request.asid, Ordering::Relaxed);
        TLB_SHOOTDOWN_START[hart].store(request.start, Ordering::Relaxed);
        TLB_SHOOTDOWN_END[hart].store(request.end, Ordering::Release);
        pending &= !(1 << hart);
    }

    publish_before_iocsr();
    pending = targets;
    while pending != 0 {
        let hart = pending.trailing_zeros() as usize;
        send_ipi(hart, IPI_TLB_SHOOTDOWN);
        pending &= !(1 << hart);
    }

    pending = targets;
    while pending != 0 {
        let hart = pending.trailing_zeros() as usize;
        while TLB_SHOOTDOWN_ACK[hart].load(Ordering::Acquire) != generation {
            poll_pending_ipi();
            core::hint::spin_loop();
        }
        TLB_SHOOTDOWN_REQUEST[hart].store(0, Ordering::Release);
        pending &= !(1 << hart);
    }
}

pub fn acknowledge_ipi() {
    let pending = iocsr_read32(IOCSR_IPI_STATUS);
    if pending != 0 {
        iocsr_write32(pending, IOCSR_IPI_CLEAR);
    }
    let hart = current_hart_id();
    if hart < MAX_HARTS {
        if pending & (1 << IPI_TLB_SHOOTDOWN) != 0 {
            let generation = TLB_SHOOTDOWN_REQUEST[hart].load(Ordering::Acquire);
            if generation != 0 {
                let request = TlbShootdownRequest {
                    kind: TLB_SHOOTDOWN_KIND[hart].load(Ordering::Relaxed),
                    asid: TLB_SHOOTDOWN_ASID[hart].load(Ordering::Relaxed),
                    start: TLB_SHOOTDOWN_START[hart].load(Ordering::Relaxed),
                    end: TLB_SHOOTDOWN_END[hart].load(Ordering::Acquire),
                };
                debug_assert!(
                    request.is_valid(),
                    "invalid published TLB shootdown request"
                );
                request.invalidate_local();
                TLB_SHOOTDOWN_ACK[hart].store(generation, Ordering::Release);
            }
        }
        IPI_COUNT[hart].fetch_add(1, Ordering::Relaxed);
    }
    crate::perf::ipi_received(1);
}
