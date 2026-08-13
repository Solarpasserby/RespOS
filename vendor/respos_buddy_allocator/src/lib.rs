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

    pub fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, ()> {
        let size = max(
            layout.size().next_power_of_two(),
            max(layout.align(), size_of::<usize>()),
        );
        let class = size.trailing_zeros() as usize;
        if class >= ORDER {
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
            self.user += layout.size();
            #[cfg(feature = "perf_counters")]
            {
                self.peak_user = self.peak_user.max(self.user);
            }
            self.allocated += size;
            return NonNull::new(ptr.cast()).ok_or(());
        }
        Err(())
    }

    pub fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        let size = max(
            layout.size().next_power_of_two(),
            max(layout.align(), size_of::<usize>()),
        );
        let mut class = size.trailing_zeros() as usize;
        let mut current = ptr.as_ptr() as usize;
        assert!(class < ORDER);
        assert!(current >= self.heap_start && current + size <= self.heap_end);
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

        self.user -= layout.size();
        self.allocated -= size;
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
    use super::{Heap, bitmap_words};
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
