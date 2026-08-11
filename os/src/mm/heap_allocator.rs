// os/src/mm/heap_allocator.rs

use crate::arch::interrupt::InterruptGuard;
use crate::config::{
    KERNEL_BASE, KERNEL_HEAP_PHYS_START, KERNEL_HEAP_SIZE, PAGE_SIZE, physical_memory_end,
};
use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use respos_buddy_allocator::{Heap, bitmap_words};
use spin::Mutex;

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
        let started = crate::perf::now_ticks();
        let mut heap = self.0.lock();
        let locked = crate::perf::now_ticks();
        let result = heap
            .alloc(layout)
            .map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr());
        let operated = crate::perf::now_ticks();
        drop(heap);
        crate::perf::heap_alloc(
            layout.size(),
            crate::perf::elapsed_since(started),
            locked.wrapping_sub(started),
            operated.wrapping_sub(locked),
            !result.is_null(),
        );
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _irq_guard = InterruptGuard::new();
        let started = crate::perf::now_ticks();
        let mut heap = self.0.lock();
        let locked = crate::perf::now_ticks();
        heap.dealloc(unsafe { NonNull::new_unchecked(ptr) }, layout);
        let operated = crate::perf::now_ticks();
        drop(heap);
        crate::perf::heap_dealloc(
            layout.size(),
            crate::perf::elapsed_since(started),
            locked.wrapping_sub(started),
            operated.wrapping_sub(locked),
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
    let heap = HEAP_ALLOCATOR.lock();
    panic!(
        "Heap allocation error, layout = {:?}, user_bytes = {}, actual_bytes = {}, total_bytes = {}",
        layout,
        heap.stats_alloc_user(),
        heap.stats_alloc_actual(),
        heap.stats_total_bytes(),
    );
}
