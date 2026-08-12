//! LoongArch QEMU-virt SMP startup and per-CPU interrupt plumbing.
//!
//! QEMU's auxiliary boot ROM parks every non-boot CPU in `idle`, waiting for
//! mailbox 0 and an IPI.  The boot CPU publishes the physical secondary entry
//! through `IOCSR_MBUF_SEND`, then sends vector 0 through `IOCSR_IPI_SEND`.

use core::{
    arch::asm,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

pub const MAX_HARTS: usize = 12;

const BOOT_COLD: u8 = 0;
const BOOT_READY: u8 = 1;
const IOCSR_IPI_STATUS: usize = 0x1000;
const IOCSR_IPI_EN: usize = 0x1004;
const IOCSR_IPI_CLEAR: usize = 0x100c;
const IOCSR_IPI_SEND: usize = 0x1040;
const IOCSR_MBUF_SEND: usize = 0x1048;
const SEND_BLOCKING: usize = 1 << 31;
const SEND_CPU_SHIFT: usize = 16;
const MBUF_BOX_SHIFT: usize = 2;
const MBUF_BUF_SHIFT: usize = 32;

static BOOT_STATE: AtomicU8 = AtomicU8::new(BOOT_COLD);
static ONLINE_HART_MASK: AtomicUsize = AtomicUsize::new(0);
static IDLE_HART_MASK: AtomicUsize = AtomicUsize::new(0);
static IPI_COUNT: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];

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
        send_ipi(hart, 0);
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
    send_ipi(hart, 0);
    crate::perf::scheduler_ipi(1);
}

pub fn acknowledge_ipi() {
    let pending = iocsr_read32(IOCSR_IPI_STATUS);
    if pending != 0 {
        iocsr_write32(pending, IOCSR_IPI_CLEAR);
    }
    let hart = current_hart_id();
    if hart < MAX_HARTS {
        IPI_COUNT[hart].fetch_add(1, Ordering::Relaxed);
    }
}
