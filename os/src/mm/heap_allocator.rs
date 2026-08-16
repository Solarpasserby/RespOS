// os/src/mm/heap_allocator.rs

use crate::arch::interrupt::InterruptGuard;
use crate::config::{
    physical_memory_end, KERNEL_BASE, KERNEL_HEAP_PHYS_START, KERNEL_HEAP_SIZE, PAGE_SIZE,
};
use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use respos_buddy_allocator::{bitmap_words, Heap};
#[cfg(feature = "heap_magazine")]
use respos_buddy_allocator::{layout_order, Magazine, MIN_ORDER};
use spin::Mutex;

#[cfg(feature = "heap_magazine")]
const MAGAZINE_MAX_ORDER: usize = 8;
#[cfg(feature = "heap_magazine")]
const MAGAZINE_CLASS_COUNT: usize = MAGAZINE_MAX_ORDER - MIN_ORDER + 1;
#[cfg(feature = "heap_magazine")]
const MAGAZINE_CLASS_CAPACITY: usize = 64;
#[cfg(feature = "heap_magazine")]
const MAGAZINE_REFILL_BATCH: usize = 16;

#[cfg(feature = "heap_magazine")]
#[repr(align(128))]
struct HartMagazine {
    state: Mutex<HartMagazineState>,
}

#[cfg(feature = "heap_magazine")]
struct HartMagazineState {
    blocks: Magazine<MAGAZINE_CLASS_COUNT>,
    allocated_user_bytes: usize,
    deallocated_user_bytes: usize,
    cached_bytes: usize,
    #[cfg(feature = "perf_counters")]
    cached_peak_bytes: usize,
}

#[cfg(feature = "heap_magazine")]
impl HartMagazine {
    const fn new() -> Self {
        Self {
            state: Mutex::new(HartMagazineState {
                blocks: Magazine::new(),
                allocated_user_bytes: 0,
                deallocated_user_bytes: 0,
                cached_bytes: 0,
                #[cfg(feature = "perf_counters")]
                cached_peak_bytes: 0,
            }),
        }
    }
}

#[cfg(feature = "heap_magazine")]
impl HartMagazineState {
    fn add_cached(&mut self, bytes: usize) {
        self.cached_bytes += bytes;
        #[cfg(feature = "perf_counters")]
        {
            self.cached_peak_bytes = self.cached_peak_bytes.max(self.cached_bytes);
        }
    }
}

#[cfg(feature = "heap_magazine")]
static HART_MAGAZINES: [HartMagazine; crate::arch::smp::MAX_HARTS] =
    [const { HartMagazine::new() }; crate::arch::smp::MAX_HARTS];

#[cfg(feature = "heap_magazine")]
fn local_magazine() -> (
    &'static HartMagazine,
    spin::MutexGuard<'static, HartMagazineState>,
) {
    let hart = crate::arch::smp::current_hart_id();
    assert!(
        hart < HART_MAGAZINES.len(),
        "heap magazine hart id out of range"
    );
    let owner = &HART_MAGAZINES[hart];
    let state = owner.state.lock();
    (owner, state)
}

#[cfg(feature = "heap_magazine")]
fn magazine_order(layout: Layout) -> Option<usize> {
    let natural = layout
        .size()
        .max(1)
        .checked_next_power_of_two()?
        .max(size_of::<usize>());
    // Keep unusually alignment-driven requests on the established buddy path.
    if layout.align() > natural {
        return None;
    }
    let order = layout_order(layout)?;
    (order <= MAGAZINE_MAX_ORDER).then_some(order)
}

#[cfg(feature = "heap_magazine")]
fn magazine_user_bytes() -> usize {
    // Statistics may run from a syscall while local interrupts are enabled.
    // Prevent interrupt-side allocation from re-entering the current hart's
    // magazine while this snapshot holds its lock.
    let _irq_guard = InterruptGuard::new();
    let (allocated, deallocated) =
        HART_MAGAZINES
            .iter()
            .fold((0usize, 0usize), |(allocated, deallocated), magazine| {
                let state = magazine.state.lock();
                (
                    allocated.wrapping_add(state.allocated_user_bytes),
                    deallocated.wrapping_add(state.deallocated_user_bytes),
                )
            });
    allocated.saturating_sub(deallocated)
}

#[cfg(feature = "heap_magazine")]
fn try_magazine_user_bytes() -> Option<usize> {
    let _irq_guard = InterruptGuard::new();
    let (allocated, deallocated) = HART_MAGAZINES.iter().try_fold(
        (0usize, 0usize),
        |(allocated, deallocated), magazine| {
            let state = magazine.state.try_lock()?;
            Some((
                allocated.wrapping_add(state.allocated_user_bytes),
                deallocated.wrapping_add(state.deallocated_user_bytes),
            ))
        },
    )?;
    Some(allocated.saturating_sub(deallocated))
}

#[cfg(feature = "heap_magazine")]
fn magazine_cached_bytes() -> usize {
    let _irq_guard = InterruptGuard::new();
    HART_MAGAZINES
        .iter()
        .map(|magazine| magazine.state.lock().cached_bytes)
        .sum()
}

#[cfg(feature = "heap_magazine")]
fn magazine_cached_peak_upper_bound() -> usize {
    #[cfg(feature = "perf_counters")]
    {
        let _irq_guard = InterruptGuard::new();
        HART_MAGAZINES
            .iter()
            .map(|magazine| magazine.state.lock().cached_peak_bytes)
            .sum()
    }
    #[cfg(not(feature = "perf_counters"))]
    {
        magazine_cached_bytes()
    }
}

/// A cross-CPU heap lock that also prevents local interrupt re-entry.
///
/// `LockedHeap` serializes different CPUs, but its internal spin lock is not
/// interrupt-aware.  Without this wrapper a timer interrupt can interrupt an
/// allocation/deallocation and allocate again on the same CPU, which spins on
/// a lock that cannot be released until the interrupt returns.
pub(crate) struct IrqSafeHeap<const ORDER: usize>(Mutex<Heap<ORDER>>);

impl<const ORDER: usize> IrqSafeHeap<ORDER> {
    pub const fn empty() -> Self {
        Self(Mutex::new(Heap::empty()))
    }

    /// Lock the heap for initialization or non-allocating statistics.
    ///
    /// The field order in `IrqSafeHeapGuard` releases the heap mutex before
    /// restoring the previous local interrupt state.
    pub fn lock(&self) -> IrqSafeHeapGuard<'_, ORDER> {
        let irq_guard = InterruptGuard::new();
        let heap_guard = self.0.lock();
        IrqSafeHeapGuard {
            heap_guard,
            _irq_guard: irq_guard,
        }
    }

    pub fn try_lock(&self) -> Option<IrqSafeHeapGuard<'_, ORDER>> {
        let irq_guard = InterruptGuard::new();
        let heap_guard = self.0.try_lock()?;
        Some(IrqSafeHeapGuard {
            heap_guard,
            _irq_guard: irq_guard,
        })
    }

    pub fn stats_alloc_user(&self) -> usize {
        #[cfg(feature = "heap_magazine")]
        {
            // Do not hold a magazine while taking the buddy lock. Allocation
            // misses and cache drains use magazine -> buddy, so reversing the
            // order here would introduce an ABBA deadlock.
            let magazine_user = magazine_user_bytes();
            let buddy_user = self.lock().stats_alloc_user();
            buddy_user.saturating_add(magazine_user)
        }
        #[cfg(not(feature = "heap_magazine"))]
        {
            self.lock().stats_alloc_user()
        }
    }

    pub fn try_stats_alloc_user(&self) -> Option<usize> {
        #[cfg(feature = "heap_magazine")]
        {
            let magazine_user = try_magazine_user_bytes()?;
            let buddy_user = self.try_lock()?.stats_alloc_user();
            Some(buddy_user.saturating_add(magazine_user))
        }
        #[cfg(not(feature = "heap_magazine"))]
        {
            Some(self.try_lock()?.stats_alloc_user())
        }
    }

    #[cfg(feature = "perf_counters")]
    pub fn perf_usage(&self) -> (usize, usize, bool) {
        #[cfg(feature = "heap_magazine")]
        {
            let magazine_user = magazine_user_bytes();
            let heap = self.lock();
            let buddy_current = heap.stats_alloc_user();
            let buddy_peak = heap.stats_peak_user();
            let current = buddy_current.saturating_add(magazine_user);
            // Exact global peak tracking would put a shared atomic back on
            // every cache hit. Report the current value and mark it inexact.
            (current, buddy_peak.max(current), false)
        }
        #[cfg(not(feature = "heap_magazine"))]
        {
            let heap = self.lock();
            let buddy_current = heap.stats_alloc_user();
            let buddy_peak = heap.stats_peak_user();
            (buddy_current, buddy_peak, true)
        }
    }

    #[cfg(feature = "perf_counters")]
    pub fn reset_perf_peak(&self) {
        self.lock().reset_peak_user();
        #[cfg(feature = "heap_magazine")]
        {
            let _irq_guard = InterruptGuard::new();
            for magazine in &HART_MAGAZINES {
                let mut state = magazine.state.lock();
                state.cached_peak_bytes = state.cached_bytes;
            }
        }
    }

    #[cfg(feature = "heap_magazine")]
    pub fn magazine_usage(&self) -> (usize, usize) {
        (magazine_cached_bytes(), magazine_cached_peak_upper_bound())
    }

    #[cfg(feature = "heap_magazine")]
    pub fn drain_magazines(&self) -> usize {
        let _irq_guard = InterruptGuard::new();
        let mut drained_blocks = 0usize;
        for owner in &HART_MAGAZINES {
            // Lock order is always magazine -> buddy, matching miss/overflow.
            // Never wait for a magazine while holding the global heap lock.
            let mut state = owner.state.lock();
            let mut heap = self.0.lock();
            for class in 0..MAGAZINE_CLASS_COUNT {
                let order = class + MIN_ORDER;
                let block_size = 1usize << order;
                let mut class_blocks = 0usize;
                state.blocks.drain_class(class, |block| {
                    // SAFETY: every magazine entry is a uniquely owned raw
                    // block reserved at this exact order.
                    unsafe { heap.dealloc_order(block, order) };
                    class_blocks += 1;
                });
                if class_blocks != 0 {
                    state.cached_bytes -= class_blocks * block_size;
                    drained_blocks += class_blocks;
                }
            }
        }
        drained_blocks
    }
}

pub(crate) struct IrqSafeHeapGuard<'a, const ORDER: usize> {
    heap_guard: spin::MutexGuard<'a, Heap<ORDER>>,
    _irq_guard: InterruptGuard,
}

impl<const ORDER: usize> Deref for IrqSafeHeapGuard<'_, ORDER> {
    type Target = Heap<ORDER>;

    fn deref(&self) -> &Self::Target {
        &self.heap_guard
    }
}

impl<const ORDER: usize> DerefMut for IrqSafeHeapGuard<'_, ORDER> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.heap_guard
    }
}

unsafe impl<const ORDER: usize> GlobalAlloc for IrqSafeHeap<ORDER> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _irq_guard = InterruptGuard::new();
        let started = crate::perf::heap_timing_start();

        #[cfg(feature = "heap_magazine")]
        if let Some(order) = magazine_order(layout) {
            let class = order - MIN_ORDER;
            let block_size = 1usize << order;
            let (_owner, mut state) = local_magazine();
            if let Some(allocation) = state.blocks.pop(class) {
                state.cached_bytes -= block_size;
                state.allocated_user_bytes = state.allocated_user_bytes.wrapping_add(layout.size());
                let operated = crate::perf::heap_timing_checkpoint(started);
                crate::perf::heap_magazine_hit(1);
                crate::perf::heap_alloc(
                    layout.size(),
                    layout.align(),
                    operated.wrapping_sub(started),
                    0,
                    operated.wrapping_sub(started),
                    true,
                    started != 0,
                );
                return allocation.as_ptr();
            }

            crate::perf::heap_magazine_miss(1);
            let mut heap = self.0.lock();
            let locked = crate::perf::heap_timing_checkpoint(started);
            let mut result = heap.alloc_order(order);
            let mut refill_blocks = 0usize;
            if result.is_ok() {
                while state.blocks.len(class) < MAGAZINE_REFILL_BATCH - 1 {
                    let Ok(extra) = heap.alloc_order(order) else {
                        break;
                    };
                    // SAFETY: `extra` is uniquely owned, word-sized/aligned,
                    // and remains reserved from `heap` until popped/drained.
                    unsafe { state.blocks.push(class, extra) };
                    state.add_cached(block_size);
                    refill_blocks += 1;
                }
            }
            let mut operated = crate::perf::heap_timing_checkpoint(started);
            drop(heap);
            let user_accounted = result.is_ok();
            if user_accounted {
                state.allocated_user_bytes = state.allocated_user_bytes.wrapping_add(layout.size());
            }
            drop(state);
            if result.is_err() {
                let reclaimed = self.drain_magazines();
                crate::perf::heap_magazine_reclaim_blocks(reclaimed);
                if reclaimed != 0 {
                    let mut heap = self.0.lock();
                    result = heap.alloc_order(order);
                    operated = crate::perf::heap_timing_checkpoint(started);
                }
            }
            if result.is_ok() && !user_accounted {
                let (_owner, mut state) = local_magazine();
                state.allocated_user_bytes = state.allocated_user_bytes.wrapping_add(layout.size());
            }
            let finished = crate::perf::heap_timing_checkpoint(started);
            crate::perf::heap_magazine_refill_blocks(refill_blocks);
            crate::perf::heap_alloc(
                layout.size(),
                layout.align(),
                finished.wrapping_sub(started),
                locked.wrapping_sub(started),
                operated.wrapping_sub(locked),
                result.is_ok(),
                started != 0,
            );
            return result.map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr());
        }

        let mut heap = self.0.lock();
        let locked = crate::perf::heap_timing_checkpoint(started);
        #[allow(unused_mut)]
        let mut result = heap.alloc(layout);
        #[allow(unused_mut)]
        let mut operated = crate::perf::heap_timing_checkpoint(started);
        drop(heap);
        #[cfg(feature = "heap_magazine")]
        if result.is_err() {
            let reclaimed = self.drain_magazines();
            crate::perf::heap_magazine_reclaim_blocks(reclaimed);
            if reclaimed != 0 {
                let mut heap = self.0.lock();
                result = heap.alloc(layout);
                operated = crate::perf::heap_timing_checkpoint(started);
            }
        }
        let finished = crate::perf::heap_timing_checkpoint(started);
        crate::perf::heap_alloc(
            layout.size(),
            layout.align(),
            finished.wrapping_sub(started),
            locked.wrapping_sub(started),
            operated.wrapping_sub(locked),
            result.is_ok(),
            started != 0,
        );
        result.map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _irq_guard = InterruptGuard::new();
        let started = crate::perf::heap_timing_start();

        #[cfg(feature = "heap_magazine")]
        if let Some(order) = magazine_order(layout) {
            let class = order - MIN_ORDER;
            let block_size = 1usize << order;
            let allocation = unsafe { NonNull::new_unchecked(ptr) };
            let (_owner, mut state) = local_magazine();
            state.deallocated_user_bytes = state.deallocated_user_bytes.wrapping_add(layout.size());
            if state.blocks.len(class) < MAGAZINE_CLASS_CAPACITY {
                // SAFETY: GlobalAlloc's contract transfers the live block back
                // exactly once; it remains reserved from the buddy allocator.
                unsafe { state.blocks.push(class, allocation) };
                state.add_cached(block_size);
                let operated = crate::perf::heap_timing_checkpoint(started);
                crate::perf::heap_magazine_cached_free(1);
                crate::perf::heap_dealloc(
                    layout.size(),
                    layout.align(),
                    operated.wrapping_sub(started),
                    0,
                    operated.wrapping_sub(started),
                    started != 0,
                );
                return;
            }

            crate::perf::heap_magazine_overflow_return(1);
            let mut heap = self.0.lock();
            let locked = crate::perf::heap_timing_checkpoint(started);
            // SAFETY: the returned block was allocated at this exact order and
            // was not inserted into the full magazine.
            unsafe { heap.dealloc_order(allocation, order) };
            let operated = crate::perf::heap_timing_checkpoint(started);
            drop(heap);
            let finished = crate::perf::heap_timing_checkpoint(started);
            crate::perf::heap_dealloc(
                layout.size(),
                layout.align(),
                finished.wrapping_sub(started),
                locked.wrapping_sub(started),
                operated.wrapping_sub(locked),
                started != 0,
            );
            return;
        }

        let mut heap = self.0.lock();
        let locked = crate::perf::heap_timing_checkpoint(started);
        heap.dealloc(unsafe { NonNull::new_unchecked(ptr) }, layout);
        let operated = crate::perf::heap_timing_checkpoint(started);
        drop(heap);
        let finished = crate::perf::heap_timing_checkpoint(started);
        crate::perf::heap_dealloc(
            layout.size(),
            layout.align(),
            finished.wrapping_sub(started),
            locked.wrapping_sub(started),
            operated.wrapping_sub(locked),
            started != 0,
        );
    }
}

#[global_allocator]
pub(crate) static HEAP_ALLOCATOR: IrqSafeHeap<32> = IrqSafeHeap::empty();

const HEAP_ORDER: usize = 32;

const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// Reserve and initialize the global heap directly after the loaded kernel.
///
/// The heap and its buddy-membership bitmap intentionally live outside the
/// ELF's BSS. The boot page tables already expose RAM through the high-half
/// direct map, so this range is usable before the final frame allocator and
/// kernel page table are constructed. The returned physical end must become
/// the frame allocator's lower bound so heap pages can never be handed out as
/// ordinary frames.
pub fn init_heap() -> usize {
    unsafe extern "C" {
        unsafe fn ekernel();
    }

    let kernel_end = ekernel as *const () as usize - KERNEL_BASE;
    let bitmap_start = if KERNEL_HEAP_PHYS_START == 0 {
        align_up(kernel_end, PAGE_SIZE)
    } else {
        assert!(
            KERNEL_HEAP_PHYS_START >= kernel_end,
            "kernel heap reservation overlaps loaded kernel"
        );
        KERNEL_HEAP_PHYS_START
    };
    let bitmap_len = bitmap_words(KERNEL_HEAP_SIZE, HEAP_ORDER)
        .checked_mul(size_of::<usize>())
        .expect("kernel heap bitmap size overflow");
    let heap_start = align_up(
        bitmap_start
            .checked_add(bitmap_len)
            .expect("kernel heap bitmap range overflow"),
        PAGE_SIZE,
    );
    let reserved_end = align_up(
        heap_start
            .checked_add(KERNEL_HEAP_SIZE)
            .expect("kernel heap range overflow"),
        PAGE_SIZE,
    );
    assert!(
        reserved_end <= physical_memory_end(),
        "insufficient RAM for kernel heap: reserved_end={:#x}, memory_end={:#x}",
        reserved_end,
        physical_memory_end(),
    );

    unsafe {
        let bitmap = core::slice::from_raw_parts_mut(
            (KERNEL_BASE + bitmap_start) as *mut usize,
            bitmap_words(KERNEL_HEAP_SIZE, HEAP_ORDER),
        );
        HEAP_ALLOCATOR
            .lock()
            .init(KERNEL_BASE + heap_start, KERNEL_HEAP_SIZE, bitmap);
    }
    reserved_end
}

#[cfg_attr(not(rust_analyzer), alloc_error_handler)]
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    let user_bytes = HEAP_ALLOCATOR.stats_alloc_user();
    let heap = HEAP_ALLOCATOR.lock();
    #[cfg(feature = "heap_magazine")]
    let cached_bytes = magazine_cached_bytes();
    #[cfg(not(feature = "heap_magazine"))]
    let cached_bytes = 0;
    panic!(
        "Heap allocation error, layout = {:?}, user_bytes = {}, buddy_reserved_bytes = {}, magazine_cached_bytes = {}, total_bytes = {}",
        layout,
        user_bytes,
        heap.stats_alloc_actual(),
        cached_bytes,
        heap.stats_total_bytes(),
    );
}
