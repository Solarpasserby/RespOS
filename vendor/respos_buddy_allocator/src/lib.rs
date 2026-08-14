#![no_std]

//! Bitmap-assisted buddy allocator for the RespOS kernel heap.
//!
//! The split/coalesce policy and accounting follow `buddy_system_allocator`
//! 0.10.0.  Free blocks of at least 16 bytes contain an intrusive doubly
//! linked node and have a membership bit, so deallocation can find and unlink
//! a free buddy in O(1).  The 8-byte class retains a singly linked list because
//! it cannot hold two pointers.

#[cfg(test)]
extern crate std;

use core::alloc::Layout;
use core::cmp::{max, min};
use core::mem::size_of;
use core::ptr::{self, NonNull};

const DOUBLE_LINK_ORDER: usize = 4;
pub const MIN_ORDER: usize = size_of::<usize>().trailing_zeros() as usize;

/// Return the buddy order needed to serve `layout`.
///
/// The returned block size is at least one machine word, the requested
/// alignment, and the next power of two covering the requested size.
pub fn layout_order(layout: Layout) -> Option<usize> {
    let requested = layout.size().max(1).checked_next_power_of_two()?;
    let size = max(requested, max(layout.align(), size_of::<usize>()));
    Some(size.trailing_zeros() as usize)
}

/// Number of machine words required by the membership bitmap.
pub const fn bitmap_words(heap_size: usize, order: usize) -> usize {
    let mut bits = 0usize;
    let mut class = DOUBLE_LINK_ORDER;
    while class < order {
        let block_size = 1usize << class;
        bits += heap_size.div_ceil(block_size) + 1;
        class += 1;
    }
    bits.div_ceil(usize::BITS as usize)
}

#[derive(Clone, Copy)]
struct FreeList {
    head: *mut usize,
}

impl FreeList {
    const fn new() -> Self {
        Self {
            head: ptr::null_mut(),
        }
    }

    fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    unsafe fn push(&mut self, ptr: *mut usize, doubly_linked: bool) {
        if doubly_linked {
            unsafe {
                ptr.write(0);
                ptr.add(1).write(self.head as usize);
                if !self.head.is_null() {
                    self.head.write(ptr as usize);
                }
            }
        } else {
            unsafe { ptr.write(self.head as usize) };
        }
        self.head = ptr;
    }

    unsafe fn pop(&mut self, doubly_linked: bool) -> Option<*mut usize> {
        let ptr = self.head;
        if ptr.is_null() {
            return None;
        }
        let next = if doubly_linked {
            unsafe { ptr.add(1).read() as *mut usize }
        } else {
            unsafe { ptr.read() as *mut usize }
        };
        self.head = next;
        if doubly_linked && !next.is_null() {
            unsafe { next.write(0) };
        }
        Some(ptr)
    }

    unsafe fn remove(&mut self, ptr: *mut usize, doubly_linked: bool) -> bool {
        if doubly_linked {
            let prev = unsafe { ptr.read() as *mut usize };
            let next = unsafe { ptr.add(1).read() as *mut usize };
            if prev.is_null() {
                if self.head != ptr {
                    return false;
                }
                self.head = next;
            } else {
                unsafe { prev.add(1).write(next as usize) };
            }
            if !next.is_null() {
                unsafe { next.write(prev as usize) };
            }
            return true;
        }

        let mut link = &mut self.head as *mut *mut usize;
        while !unsafe { *link }.is_null() {
            let current = unsafe { *link };
            if current == ptr {
                unsafe { *link = current.read() as *mut usize };
                return true;
            }
            link = current.cast();
        }
        false
    }
}

unsafe impl Send for FreeList {}

/// Intrusive LIFO storage for allocator-owned blocks.
///
/// Capacity and refill/drain policy deliberately live in the caller. A block
/// in this structure remains reserved from the buddy allocator and stores its
/// next pointer in the first machine word.
pub struct Magazine<const CLASSES: usize> {
    heads: [*mut usize; CLASSES],
    lengths: [usize; CLASSES],
}

impl<const CLASSES: usize> Magazine<CLASSES> {
    pub const fn new() -> Self {
        Self {
            heads: [ptr::null_mut(); CLASSES],
            lengths: [0; CLASSES],
        }
    }

    pub fn len(&self, class: usize) -> usize {
        self.lengths[class]
    }

    /// Insert an allocator-owned block into one class.
    ///
    /// # Safety
    ///
    /// The block must be at least one machine word, suitably aligned, uniquely
    /// owned, and remain reserved from its backing allocator while cached.
    pub unsafe fn push(&mut self, class: usize, ptr: NonNull<u8>) {
        let slot = ptr.as_ptr().cast::<usize>();
        unsafe { slot.write(self.heads[class] as usize) };
        self.heads[class] = slot;
        self.lengths[class] += 1;
    }

    pub fn pop(&mut self, class: usize) -> Option<NonNull<u8>> {
        let slot = self.heads[class];
        if slot.is_null() {
            return None;
        }
        self.heads[class] = unsafe { slot.read() as *mut usize };
        self.lengths[class] -= 1;
        NonNull::new(slot.cast())
    }

    pub fn drain_class(&mut self, class: usize, mut return_block: impl FnMut(NonNull<u8>)) {
        while let Some(block) = self.pop(class) {
            return_block(block);
        }
    }
}

unsafe impl<const CLASSES: usize> Send for Magazine<CLASSES> {}

pub struct Heap<const ORDER: usize> {
    free_list: [FreeList; ORDER],
    user: usize,
    #[cfg(feature = "perf_counters")]
    peak_user: usize,
    allocated: usize,
    total: usize,
    heap_start: usize,
    heap_end: usize,
    bitmap: *mut usize,
    bitmap_words: usize,
}

unsafe impl<const ORDER: usize> Send for Heap<ORDER> {}

impl<const ORDER: usize> Heap<ORDER> {
    pub const fn empty() -> Self {
        Self {
            free_list: [FreeList::new(); ORDER],
            user: 0,
            #[cfg(feature = "perf_counters")]
            peak_user: 0,
            allocated: 0,
            total: 0,
            heap_start: 0,
            heap_end: 0,
            bitmap: ptr::null_mut(),
            bitmap_words: 0,
        }
    }

    /// Initialize one heap region and its caller-owned membership bitmap.
    ///
    /// # Safety
    ///
    /// Both regions must be uniquely owned for the lifetime of this heap.
    pub unsafe fn init(&mut self, start: usize, size: usize, bitmap: &'static mut [usize]) {
        assert!(self.total == 0);
        assert!(bitmap.len() >= bitmap_words(size, ORDER));
        bitmap.fill(0);
        self.heap_start = start;
        self.heap_end = start.checked_add(size).expect("heap range overflow");
        self.bitmap = bitmap.as_mut_ptr();
        self.bitmap_words = bitmap.len();
        unsafe { self.add_to_heap(start, self.heap_end) };
    }

    unsafe fn add_to_heap(&mut self, mut start: usize, mut end: usize) {
        start = start.next_multiple_of(size_of::<usize>());
        end -= end % size_of::<usize>();
        assert!(start <= end);
        let mut current = start;
        while current + size_of::<usize>() <= end {
            let lowbit = current & current.wrapping_neg();
            let remaining_order = (usize::BITS - 1 - (end - current).leading_zeros()) as usize;
            let order = min(lowbit.trailing_zeros() as usize, remaining_order);
            unsafe { self.push_free(order, current as *mut usize) };
            self.total += 1usize << order;
            current += 1usize << order;
        }
    }

    fn bitmap_index(&self, class: usize, ptr: usize) -> Option<usize> {
        if class < DOUBLE_LINK_ORDER || class >= ORDER {
            return None;
        }
        if ptr < self.heap_start || ptr >= self.heap_end {
            return None;
        }
        let heap_size = self.heap_end - self.heap_start;
        let mut base = 0usize;
        let mut order = DOUBLE_LINK_ORDER;
        while order < class {
            base += heap_size.div_ceil(1usize << order) + 1;
            order += 1;
        }
        Some(base + (ptr - self.heap_start) / (1usize << class))
    }

    fn bitmap_get(&self, class: usize, ptr: usize) -> bool {
        let Some(bit) = self.bitmap_index(class, ptr) else {
            return false;
        };
        let word = bit / usize::BITS as usize;
        let shift = bit % usize::BITS as usize;
        assert!(word < self.bitmap_words);
        unsafe { (*self.bitmap.add(word) & (1usize << shift)) != 0 }
    }

    fn bitmap_set(&mut self, class: usize, ptr: usize, free: bool) {
        let Some(bit) = self.bitmap_index(class, ptr) else {
            return;
        };
        let word = bit / usize::BITS as usize;
        let mask = 1usize << (bit % usize::BITS as usize);
        assert!(word < self.bitmap_words);
        let slot = unsafe { &mut *self.bitmap.add(word) };
        if free {
            assert_eq!(*slot & mask, 0, "free block inserted twice");
            *slot |= mask;
        } else {
            assert_ne!(*slot & mask, 0, "free block membership missing");
            *slot &= !mask;
        }
    }

    unsafe fn push_free(&mut self, class: usize, ptr: *mut usize) {
        unsafe { self.free_list[class].push(ptr, class >= DOUBLE_LINK_ORDER) };
        self.bitmap_set(class, ptr as usize, true);
    }

    unsafe fn pop_free(&mut self, class: usize) -> Option<*mut usize> {
        let ptr = unsafe { self.free_list[class].pop(class >= DOUBLE_LINK_ORDER) }?;
        self.bitmap_set(class, ptr as usize, false);
        Some(ptr)
    }

    unsafe fn remove_free(&mut self, class: usize, ptr: *mut usize) -> bool {
        let removed = unsafe { self.free_list[class].remove(ptr, class >= DOUBLE_LINK_ORDER) };
        if removed {
            self.bitmap_set(class, ptr as usize, false);
        }
        removed
    }

    /// Reserve one block of exactly `1 << class` bytes without changing the
    /// caller-requested-byte accounting. This is the ownership-transfer API
    /// used by bounded caches layered above the buddy allocator.
    pub fn alloc_order(&mut self, class: usize) -> Result<NonNull<u8>, ()> {
        if class < MIN_ORDER || class >= ORDER {
            return Err(());
        }
        for source_class in class..ORDER {
            if self.free_list[source_class].is_empty() {
                continue;
            }
            for split_class in (class + 1..=source_class).rev() {
                let block = unsafe { self.pop_free(split_class).ok_or(())? };
                let half = 1usize << (split_class - 1);
                unsafe {
                    self.push_free(split_class - 1, block);
                    self.push_free(split_class - 1, (block as usize + half) as *mut usize);
                }
            }
            let ptr = unsafe { self.pop_free(class).ok_or(())? };
            self.allocated += 1usize << class;
            return NonNull::new(ptr.cast()).ok_or(());
        }
        Err(())
    }

    /// Return a block previously obtained from `alloc_order` to the buddy.
    ///
    /// # Safety
    ///
    /// `ptr` must denote a currently reserved block of exactly `1 << class`
    /// bytes from this heap. It must not be returned twice or used afterwards.
    pub unsafe fn dealloc_order(&mut self, ptr: NonNull<u8>, mut class: usize) {
        assert!(class >= MIN_ORDER && class < ORDER);
        let size = 1usize << class;
        let mut current = ptr.as_ptr() as usize;
        assert!(current >= self.heap_start && current + size <= self.heap_end);
        assert_eq!(current & (size - 1), 0, "buddy block is misaligned");
        unsafe { self.push_free(class, current as *mut usize) };

        while class + 1 < ORDER {
            let buddy = current ^ (1usize << class);
            let buddy_free = if class >= DOUBLE_LINK_ORDER {
                self.bitmap_get(class, buddy)
            } else {
                // The 8-byte node has no room for a back pointer.
                let mut found = false;
                let mut cursor = self.free_list[class].head;
                while !cursor.is_null() {
                    if cursor as usize == buddy {
                        found = true;
                        break;
                    }
                    cursor = unsafe { cursor.read() as *mut usize };
                }
                found
            };
            if !buddy_free {
                break;
            }
            assert!(unsafe { self.remove_free(class, current as *mut usize) });
            assert!(unsafe { self.remove_free(class, buddy as *mut usize) });
            current = min(current, buddy);
            class += 1;
            unsafe { self.push_free(class, current as *mut usize) };
        }

        self.allocated -= size;
    }

    pub fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, ()> {
        let class = layout_order(layout).ok_or(())?;
        let ptr = self.alloc_order(class)?;
        self.user += layout.size();
        #[cfg(feature = "perf_counters")]
        {
            self.peak_user = self.peak_user.max(self.user);
        }
        Ok(ptr)
    }

    pub fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        let class = layout_order(layout).expect("valid allocated layout must have a buddy order");
        self.user -= layout.size();
        unsafe { self.dealloc_order(ptr, class) };
    }

    pub fn stats_alloc_user(&self) -> usize {
        self.user
    }

    #[cfg(feature = "perf_counters")]
    pub fn stats_peak_user(&self) -> usize {
        self.peak_user
    }

    #[cfg(feature = "perf_counters")]
    pub fn reset_peak_user(&mut self) {
        self.peak_user = self.user;
    }

    pub fn stats_alloc_actual(&self) -> usize {
        self.allocated
    }

    pub fn stats_total_bytes(&self) -> usize {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::{Heap, MIN_ORDER, Magazine, bitmap_words, layout_order};
    use core::alloc::Layout;
    use std::alloc::{alloc_zeroed, dealloc};
    use std::boxed::Box;
    use std::vec::Vec;

    const ORDER: usize = 24;
    const HEAP_SIZE: usize = 1 << 20;

    fn new_heap() -> (Heap<ORDER>, *mut u8, Layout) {
        let backing_layout = Layout::from_size_align(HEAP_SIZE, HEAP_SIZE).unwrap();
        let backing = unsafe { alloc_zeroed(backing_layout) };
        assert!(!backing.is_null());
        let bitmap = std::vec![0usize; bitmap_words(HEAP_SIZE, ORDER)].into_boxed_slice();
        let bitmap = Box::leak(bitmap);
        let mut heap = Heap::empty();
        unsafe { heap.init(backing as usize, HEAP_SIZE, bitmap) };
        (heap, backing, backing_layout)
    }

    #[test]
    fn layouts_remain_aligned_and_fully_coalesce() {
        let (mut heap, backing, backing_layout) = new_heap();
        let layouts = [
            Layout::from_size_align(1, 1).unwrap(),
            Layout::from_size_align(8, 8).unwrap(),
            Layout::from_size_align(17, 32).unwrap(),
            Layout::from_size_align(4096, 4096).unwrap(),
            Layout::from_size_align(65537, 16).unwrap(),
        ];
        let mut live = Vec::new();
        for layout in layouts.into_iter().cycle().take(256) {
            let Ok(ptr) = heap.alloc(layout) else { break };
            assert_eq!(ptr.as_ptr() as usize % layout.align(), 0);
            let range = ptr.as_ptr() as usize..ptr.as_ptr() as usize + layout.size();
            assert!(
                live.iter()
                    .all(
                        |(other, _): &(core::ops::Range<usize>, Layout)| range.end <= other.start
                            || range.start >= other.end
                    )
            );
            live.push((range, layout));
        }
        assert!(!live.is_empty());
        while let Some((range, layout)) = live.pop() {
            heap.dealloc(
                unsafe { core::ptr::NonNull::new_unchecked(range.start as *mut u8) },
                layout,
            );
        }
        assert_eq!(heap.stats_alloc_user(), 0);
        assert_eq!(heap.stats_alloc_actual(), 0);

        let large = Layout::from_size_align(HEAP_SIZE / 2, HEAP_SIZE / 2).unwrap();
        let ptr = heap.alloc(large).expect("fully freed heap must coalesce");
        heap.dealloc(ptr, large);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn mixed_order_freeing_preserves_accounting() {
        let (mut heap, backing, backing_layout) = new_heap();
        let mut live = Vec::new();
        for index in 0..4096usize {
            let size = 1 + ((index.wrapping_mul(1103515245).wrapping_add(12345)) & 2047);
            let align = 1usize << (index % 9);
            let layout = Layout::from_size_align(size, align).unwrap();
            if let Ok(ptr) = heap.alloc(layout) {
                live.push((ptr, layout));
            }
            if index % 3 == 0 && !live.is_empty() {
                let slot = index % live.len();
                let (ptr, layout) = live.swap_remove(slot);
                heap.dealloc(ptr, layout);
            }
        }
        for (ptr, layout) in live.drain(..).rev() {
            heap.dealloc(ptr, layout);
        }
        assert_eq!(heap.stats_alloc_user(), 0);
        assert_eq!(heap.stats_alloc_actual(), 0);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn raw_order_transfer_preserves_user_accounting_and_coalesces() {
        let (mut heap, backing, backing_layout) = new_heap();
        let order = layout_order(Layout::from_size_align(17, 8).unwrap()).unwrap();
        assert_eq!(order, 5);
        let mut blocks = Vec::new();
        for _ in 0..128 {
            let ptr = heap.alloc_order(order).unwrap();
            assert_eq!(ptr.as_ptr() as usize % (1usize << order), 0);
            blocks.push(ptr);
        }
        assert_eq!(heap.stats_alloc_user(), 0);
        assert_eq!(heap.stats_alloc_actual(), 128 * (1usize << order));

        for ptr in blocks.drain(..).rev() {
            unsafe { heap.dealloc_order(ptr, order) };
        }
        assert_eq!(heap.stats_alloc_user(), 0);
        assert_eq!(heap.stats_alloc_actual(), 0);

        let large = Layout::from_size_align(HEAP_SIZE / 2, HEAP_SIZE / 2).unwrap();
        let ptr = heap.alloc(large).expect("raw blocks must fully coalesce");
        heap.dealloc(ptr, large);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn invalid_raw_orders_fail_without_changing_accounting() {
        let (mut heap, backing, backing_layout) = new_heap();
        assert!(heap.alloc_order(0).is_err());
        assert!(heap.alloc_order(ORDER).is_err());
        assert_eq!(heap.stats_alloc_user(), 0);
        assert_eq!(heap.stats_alloc_actual(), 0);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn magazine_is_bounded_by_its_caller_and_preserves_lifo_ownership() {
        let (mut heap, backing, backing_layout) = new_heap();
        let order = 5;
        let mut magazine = Magazine::<2>::new();
        let first = heap.alloc_order(order).unwrap();
        let second = heap.alloc_order(order).unwrap();
        unsafe {
            magazine.push(0, first);
            magazine.push(0, second);
        }
        assert_eq!(magazine.len(0), 2);
        let mut drained = Vec::new();
        magazine.drain_class(0, |block| drained.push(block));
        assert_eq!(drained, [second, first]);
        assert_eq!(magazine.len(0), 0);
        for block in drained {
            unsafe { heap.dealloc_order(block, order) };
        }
        assert_eq!(heap.stats_alloc_actual(), 0);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn magazines_cover_all_small_orders_cross_owner_and_fully_coalesce() {
        const CLASSES: usize = 6;
        const CAPACITY: usize = 8;
        let (mut heap, backing, backing_layout) = new_heap();
        let mut source = Magazine::<CLASSES>::new();
        let mut remote = Magazine::<CLASSES>::new();

        for class in 0..CLASSES {
            let order = MIN_ORDER + class;
            for _ in 0..CAPACITY {
                let block = heap.alloc_order(order).unwrap();
                unsafe { source.push(class, block) };
            }

            // The caller enforces the hard capacity and returns overflow to
            // the authoritative buddy allocator.
            let overflow = heap.alloc_order(order).unwrap();
            unsafe { heap.dealloc_order(overflow, order) };

            // Popping on one simulated hart and freeing into another covers
            // the ownership transfer used by cross-hart deallocation.
            while let Some(block) = source.pop(class) {
                unsafe { remote.push(class, block) };
            }
            assert_eq!(source.len(class), 0);
            assert_eq!(remote.len(class), CAPACITY);
            remote.drain_class(class, |block| unsafe { heap.dealloc_order(block, order) });
        }

        assert_eq!(heap.stats_alloc_user(), 0);
        assert_eq!(heap.stats_alloc_actual(), 0);
        let large = Layout::from_size_align(HEAP_SIZE / 2, HEAP_SIZE / 2).unwrap();
        let ptr = heap.alloc(large).expect("drained magazines must coalesce");
        heap.dealloc(ptr, large);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn raw_orders_preserve_mixed_layout_alignment() {
        let (mut heap, backing, backing_layout) = new_heap();
        let sizes = [1, 7, 8, 9, 17, 33, 129, 256];
        let alignments = [1, 8, 16, 32, 64, 128, 256];
        for size in sizes {
            for align in alignments {
                let layout = Layout::from_size_align(size, align).unwrap();
                let order = layout_order(layout).unwrap();
                let block = heap.alloc_order(order).unwrap();
                assert_eq!(block.as_ptr() as usize % align, 0);
                unsafe { heap.dealloc_order(block, order) };
            }
        }
        assert_eq!(heap.stats_alloc_actual(), 0);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn partial_refill_oom_drain_and_retry_preserve_every_block() {
        let (mut heap, backing, backing_layout) = new_heap();
        let mut large = Vec::new();
        for _ in 0..(HEAP_SIZE / 256 - 1) {
            large.push(heap.alloc_order(8).unwrap());
        }

        // Exactly two minimum-order blocks remain after these reservations.
        let mut blockers = Vec::new();
        for _ in 0..30 {
            blockers.push(heap.alloc_order(MIN_ORDER).unwrap());
        }
        let live = heap.alloc_order(MIN_ORDER).unwrap();
        let mut magazine = Magazine::<1>::new();
        let cached = heap.alloc_order(MIN_ORDER).unwrap();
        unsafe { magazine.push(0, cached) };
        assert!(heap.alloc_order(MIN_ORDER).is_err());

        // This mirrors the production OOM recovery: drain caches, then retry
        // once. The retry must observe the returned block immediately.
        magazine.drain_class(0, |block| unsafe { heap.dealloc_order(block, MIN_ORDER) });
        let retried = heap
            .alloc_order(MIN_ORDER)
            .expect("drained block must satisfy retry");

        unsafe {
            heap.dealloc_order(retried, MIN_ORDER);
            heap.dealloc_order(live, MIN_ORDER);
        }
        for block in blockers {
            unsafe { heap.dealloc_order(block, MIN_ORDER) };
        }
        for block in large {
            unsafe { heap.dealloc_order(block, 8) };
        }
        assert_eq!(heap.stats_alloc_actual(), 0);

        let whole = Layout::from_size_align(HEAP_SIZE / 2, HEAP_SIZE / 2).unwrap();
        let ptr = heap
            .alloc(whole)
            .expect("OOM recovery must not fragment heap");
        heap.dealloc(ptr, whole);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[test]
    fn randomized_magazine_transfers_do_not_duplicate_or_lose_blocks() {
        const CLASSES: usize = 6;
        const CAPACITY: usize = 16;
        let (mut heap, backing, backing_layout) = new_heap();
        let mut magazines = [Magazine::<CLASSES>::new(), Magazine::<CLASSES>::new()];
        let mut live = Vec::new();
        let mut state = 0x9e37_79b9_7f4a_7c15usize;

        for _ in 0..20_000 {
            state ^= state << 7;
            state ^= state >> 9;
            state ^= state << 8;
            if live.is_empty() || state & 1 == 0 {
                let order = MIN_ORDER + (state >> 1) % CLASSES;
                let hart = (state >> 8) & 1;
                let class = order - MIN_ORDER;
                let block = magazines[hart]
                    .pop(class)
                    .or_else(|| heap.alloc_order(order).ok());
                if let Some(block) = block {
                    live.push((block, order));
                }
            } else {
                let index = (state >> 16) % live.len();
                let (block, order) = live.swap_remove(index);
                let hart = (state >> 24) & 1;
                let class = order - MIN_ORDER;
                if magazines[hart].len(class) < CAPACITY {
                    unsafe { magazines[hart].push(class, block) };
                } else {
                    unsafe { heap.dealloc_order(block, order) };
                }
            }
        }

        for (block, order) in live {
            unsafe { heap.dealloc_order(block, order) };
        }
        for magazine in &mut magazines {
            for class in 0..CLASSES {
                let order = MIN_ORDER + class;
                magazine.drain_class(class, |block| unsafe { heap.dealloc_order(block, order) });
            }
        }
        assert_eq!(heap.stats_alloc_user(), 0);
        assert_eq!(heap.stats_alloc_actual(), 0);
        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }

    #[cfg(feature = "perf_counters")]
    #[test]
    fn peak_user_can_be_rebased_without_changing_live_accounting() {
        let (mut heap, backing, backing_layout) = new_heap();
        let first_layout = Layout::from_size_align(24, 8).unwrap();
        let second_layout = Layout::from_size_align(80, 16).unwrap();
        let first = heap.alloc(first_layout).unwrap();
        assert_eq!(heap.stats_peak_user(), 24);
        heap.reset_peak_user();
        assert_eq!(heap.stats_alloc_user(), 24);
        assert_eq!(heap.stats_peak_user(), 24);

        let second = heap.alloc(second_layout).unwrap();
        assert_eq!(heap.stats_alloc_user(), 104);
        assert_eq!(heap.stats_peak_user(), 104);
        heap.dealloc(second, second_layout);
        assert_eq!(heap.stats_alloc_user(), 24);
        assert_eq!(heap.stats_peak_user(), 104);
        heap.reset_peak_user();
        assert_eq!(heap.stats_peak_user(), 24);
        heap.dealloc(first, first_layout);

        drop(heap);
        unsafe { dealloc(backing, backing_layout) };
    }
}
