// os/src/drivers/virtio.rs

mod block_dev;
mod net_dev;

pub use block_dev::VirtIoBlkDev;
pub use net_dev::VirtIoNetDev;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::NonNull;
use lazy_static::*;
use spin::Mutex;
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

use crate::arch::{mm::PageTable, read_mmu_token};
use crate::config::{KERNEL_BASE, PAGE_SIZE, physical_memory_end};
use crate::mm::{
    FrameTracker, PhysAddr as KernelPA, PhysPageNum as KernelPPN, VirtAddr, frame_alloc,
};

lazy_static! {
    /// Holds DMA allocations alive until virtio-drivers calls dma_dealloc.
    ///
    /// Each inner Vec is one contiguous allocation returned by dma_alloc(pages).
    static ref DMA_ALLOCATIONS: Mutex<Vec<Vec<FrameTracker>>> = Mutex::new(Vec::new());
    /// Active copies for virtually contiguous buffers whose physical pages are
    /// not contiguous. VirtIO descriptors can name only one physical range.
    static ref SHARED_BOUNCES: Mutex<BounceBuffers> = Mutex::new(BounceBuffers::new());
}

struct SharedBounce {
    paddr: PhysAddr,
    bytes: Box<[u8]>,
}

struct BounceBuffers {
    active: Vec<SharedBounce>,
    free: Vec<Box<[u8]>>,
    cached_bytes: usize,
    #[cfg(feature = "perf_counters")]
    perf: BouncePerfSnapshot,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct BouncePerfSnapshot {
    pub calls: usize,
    pub bytes: usize,
    pub copy_to_device_bytes: usize,
    pub copy_from_device_bytes: usize,
    pub allocations: usize,
    pub cache_hits: usize,
    pub share_ticks: usize,
    pub unshare_ticks: usize,
    pub active_peak: usize,
    pub active: usize,
    pub cached_buffers: usize,
    pub cached_bytes: usize,
}

impl BounceBuffers {
    const MAX_CACHED: usize = 64;
    const MAX_CACHED_BYTES: usize = 1024 * 1024;

    const fn new() -> Self {
        Self {
            active: Vec::new(),
            free: Vec::new(),
            cached_bytes: 0,
            #[cfg(feature = "perf_counters")]
            perf: BouncePerfSnapshot {
                calls: 0,
                bytes: 0,
                copy_to_device_bytes: 0,
                copy_from_device_bytes: 0,
                allocations: 0,
                cache_hits: 0,
                share_ticks: 0,
                unshare_ticks: 0,
                active_peak: 0,
                active: 0,
                cached_buffers: 0,
                cached_bytes: 0,
            },
        }
    }

    fn take_free(&mut self, len: usize) -> Option<Box<[u8]>> {
        let index = self.free.iter().position(|buffer| buffer.len() == len)?;
        let buffer = self.free.swap_remove(index);
        self.cached_bytes = self
            .cached_bytes
            .checked_sub(buffer.len())
            .expect("virtio bounce cache byte accounting underflow");
        Some(buffer)
    }

    fn recycle(&mut self, buffer: Box<[u8]>) -> Option<Box<[u8]>> {
        let new_cached_bytes = self.cached_bytes.checked_add(buffer.len());
        if self.free.len() < Self::MAX_CACHED
            && new_cached_bytes.is_some_and(|bytes| bytes <= Self::MAX_CACHED_BYTES)
        {
            self.cached_bytes = new_cached_bytes.unwrap();
            self.free.push(buffer);
            None
        } else {
            Some(buffer)
        }
    }
}

pub(crate) fn snapshot_bounce_perf() -> BouncePerfSnapshot {
    #[cfg(feature = "perf_counters")]
    {
        let buffers = SHARED_BOUNCES.lock();
        BouncePerfSnapshot {
            active: buffers.active.len(),
            cached_buffers: buffers.free.len(),
            cached_bytes: buffers.cached_bytes,
            ..buffers.perf
        }
    }
    #[cfg(not(feature = "perf_counters"))]
    BouncePerfSnapshot::default()
}

#[cfg(feature = "perf_counters")]
pub(crate) fn reset_bounce_perf() {
    SHARED_BOUNCES.lock().perf = BouncePerfSnapshot::default();
}

pub struct VirtIoHalImpl;

impl VirtIoHalImpl {
    fn virt_to_phys(vaddr: usize) -> PhysAddr {
        let direct_map_start = KERNEL_BASE;
        let direct_map_end = KERNEL_BASE + physical_memory_end();
        if (direct_map_start..direct_map_end).contains(&vaddr) {
            vaddr - KERNEL_BASE
        } else {
            let page_table = PageTable::from_token(read_mmu_token());
            let pa = page_table
                .translate_va(VirtAddr::from(vaddr))
                .expect("[kernel] virtio share: address is not mapped");
            usize::from(pa)
        }
    }

    /// Return the physical start when every page covered by the virtual range
    /// is adjacent in physical memory. A descriptor may otherwise reach the
    /// wrong frame after crossing a page boundary.
    fn contiguous_phys(vaddr: usize, len: usize) -> Option<PhysAddr> {
        assert_ne!(len, 0, "[kernel] virtio share: empty buffer");
        let end = vaddr
            .checked_add(len - 1)
            .expect("[kernel] virtio share: address overflow");

        let direct_map_start = KERNEL_BASE;
        let direct_map_end = KERNEL_BASE + physical_memory_end();
        if vaddr >= direct_map_start && end < direct_map_end {
            return Some(vaddr - KERNEL_BASE);
        }

        let first_paddr = Self::virt_to_phys(vaddr);
        let first_page_end = (vaddr | (PAGE_SIZE - 1))
            .checked_add(1)
            .unwrap_or(usize::MAX);
        if end < first_page_end {
            return Some(first_paddr);
        }

        let page_table = PageTable::from_token(read_mmu_token());
        let mut page_vaddr = first_page_end;
        while page_vaddr <= end {
            let paddr = page_table
                .translate_va(VirtAddr::from(page_vaddr))
                .expect("[kernel] virtio share: range is not fully mapped");
            let expected = first_paddr.checked_add(page_vaddr - vaddr)?;
            if usize::from(paddr) != expected {
                return None;
            }
            page_vaddr = page_vaddr.checked_add(PAGE_SIZE)?;
        }
        Some(first_paddr)
    }
}

unsafe impl Hal for VirtIoHalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        assert!(pages > 0, "[kernel] dma_alloc: pages must be non-zero");

        let mut ppn_base = KernelPPN(0);
        let mut frames = Vec::new();

        for i in 0..pages {
            let frame = frame_alloc().expect("[kernel] dma_alloc: frame allocation failed");
            let ppn = frame.ppn();

            if i == 0 {
                ppn_base = ppn;
            }

            // TODO: 当前栈式页帧分配器无法保证分配连续物理页帧
            assert_eq!(
                ppn.0,
                ppn_base.0 + i,
                "[kernel] dma_alloc: allocated frames are not contiguous"
            );

            frames.push(frame);
        }

        DMA_ALLOCATIONS.lock().push(frames);

        let pa = KernelPA::from(ppn_base);
        let va = VirtAddr::from(pa.0 + KERNEL_BASE);
        let vaddr =
            NonNull::new(usize::from(va) as *mut u8).expect("dma_alloc: null virtual address");

        (usize::from(pa), vaddr)
    }

    unsafe fn dma_dealloc(pa: PhysAddr, _va: NonNull<u8>, pages: usize) -> i32 {
        let pa = KernelPA::from(pa);
        let ppn_base = KernelPPN::from(pa);

        let frames = {
            let mut allocations = DMA_ALLOCATIONS.lock();

            let index = allocations
                .iter()
                .position(|allocation| {
                    allocation
                        .first()
                        .map(|frame| frame.ppn() == ppn_base)
                        .unwrap_or(false)
                })
                .expect("dma_dealloc: allocation not found");

            allocations.swap_remove(index)
        };

        assert_eq!(frames.len(), pages, "dma_dealloc: page count mismatch");

        // Dropping FrameTracker returns each physical frame to the frame allocator.
        drop(frames);

        0
    }

    unsafe fn mmio_phys_to_virt(pa: PhysAddr, _size: usize) -> NonNull<u8> {
        let va = pa + KERNEL_BASE;
        NonNull::new(va as *mut u8).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        let vaddr = buffer.as_ptr() as *mut u8 as usize;
        let len = buffer.len();
        if let Some(paddr) = Self::contiguous_phys(vaddr, len) {
            return paddr;
        }

        #[cfg(feature = "perf_counters")]
        let started = crate::perf::now_ticks();
        let cached = SHARED_BOUNCES.lock().take_free(len);
        #[cfg(feature = "perf_counters")]
        let cache_hit = cached.is_some();
        let mut bounce = cached.unwrap_or_else(|| alloc::vec![0; len].into_boxed_slice());
        let copies_to_device = matches!(
            direction,
            BufferDirection::DriverToDevice | BufferDirection::Both
        );
        if copies_to_device {
            // SAFETY: Hal::share guarantees a valid non-empty source range,
            // and `bounce` is a distinct allocation of exactly the same size.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer.as_ptr() as *const u8,
                    bounce.as_mut_ptr(),
                    len,
                );
            }
        }

        // Kernel heap allocations live in the linear physical direct map, so
        // the boxed bounce range is physically contiguous even across pages.
        let paddr = Self::virt_to_phys(bounce.as_ptr() as usize);
        let mut buffers = SHARED_BOUNCES.lock();
        buffers.active.push(SharedBounce {
            paddr,
            bytes: bounce,
        });
        #[cfg(feature = "perf_counters")]
        {
            buffers.perf.calls += 1;
            buffers.perf.bytes += len;
            if copies_to_device {
                buffers.perf.copy_to_device_bytes += len;
            }
            if cache_hit {
                buffers.perf.cache_hits += 1;
            } else {
                buffers.perf.allocations += 1;
            }
            buffers.perf.share_ticks += crate::perf::elapsed_since(started);
            buffers.perf.active_peak = buffers.perf.active_peak.max(buffers.active.len());
        }
        paddr
    }

    unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        #[cfg(feature = "perf_counters")]
        let started = crate::perf::now_ticks();
        let bounce = {
            let mut buffers = SHARED_BOUNCES.lock();
            let Some(index) = buffers
                .active
                .iter()
                .position(|bounce| bounce.paddr == paddr)
            else {
                // Physically contiguous buffers were shared without copying.
                return;
            };
            buffers.active.swap_remove(index).bytes
        };

        let len = buffer.len();
        assert_eq!(bounce.len(), len, "virtio unshare: buffer length changed");
        let copies_from_device = matches!(
            direction,
            BufferDirection::DeviceToDriver | BufferDirection::Both
        );
        if copies_from_device {
            // SAFETY: Hal::unshare guarantees a valid destination range which
            // is no longer accessed by the device.
            unsafe {
                core::ptr::copy_nonoverlapping(bounce.as_ptr(), buffer.as_ptr() as *mut u8, len);
            }
        }
        let mut buffers = SHARED_BOUNCES.lock();
        let uncached = buffers.recycle(bounce);
        #[cfg(feature = "perf_counters")]
        {
            if copies_from_device {
                buffers.perf.copy_from_device_bytes += len;
            }
            buffers.perf.unshare_ticks += crate::perf::elapsed_since(started);
        }
        drop(buffers);
        drop(uncached);
    }
}
