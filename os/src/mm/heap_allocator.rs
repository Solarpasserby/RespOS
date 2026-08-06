// os/src/mm/heap_allocator.rs

use crate::arch::interrupt::InterruptGuard;
use crate::config::KERNEL_HEAP_SIZE;
use buddy_system_allocator::{Heap, LockedHeap};
use core::alloc::{GlobalAlloc, Layout};
use core::ops::{Deref, DerefMut};

/// A cross-CPU heap lock that also prevents local interrupt re-entry.
///
/// `LockedHeap` serializes different CPUs, but its internal spin lock is not
/// interrupt-aware.  Without this wrapper a timer interrupt can interrupt an
/// allocation/deallocation and allocate again on the same CPU, which spins on
/// a lock that cannot be released until the interrupt returns.
pub(crate) struct IrqSafeHeap<const ORDER: usize>(LockedHeap<ORDER>);

impl<const ORDER: usize> IrqSafeHeap<ORDER> {
    pub const fn empty() -> Self {
        Self(LockedHeap::empty())
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
        unsafe { GlobalAlloc::alloc(&self.0, layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _irq_guard = InterruptGuard::new();
        unsafe { GlobalAlloc::dealloc(&self.0, ptr, layout) }
    }
}

#[global_allocator]
pub(crate) static HEAP_ALLOCATOR: IrqSafeHeap<32> = IrqSafeHeap::empty();

// .bss 段上存放内核堆
static mut HEAP_SPACE: [u8; KERNEL_HEAP_SIZE] = [0; KERNEL_HEAP_SIZE];

/// 初始化全局堆分配器
pub fn init_heap() {
    unsafe {
        HEAP_ALLOCATOR
            .lock()
            .init((&raw mut HEAP_SPACE) as usize, KERNEL_HEAP_SIZE);
    }
}

#[cfg_attr(not(rust_analyzer), alloc_error_handler)]
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}
