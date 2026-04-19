// os/src/drivers/virtio.rs

mod block_dev;

pub use block_dev::VirtIoBlkDev;

use alloc::vec::Vec;
use core::ptr::NonNull;
use lazy_static::*;
use spin::Mutex;
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

use crate::config::KERNEL_BASE;
use crate::mm::{
    frame_alloc,
    FrameTracker,
    PhysAddr as KernelPA,
    PhysPageNum as KernelPPN,
    VirtAddr,
};

lazy_static! {
    /// Holds DMA allocations alive until virtio-drivers calls dma_dealloc.
    ///
    /// Each inner Vec is one contiguous allocation returned by dma_alloc(pages).
    static ref DMA_ALLOCATIONS: Mutex<Vec<Vec<FrameTracker>>> = Mutex::new(Vec::new());
}

pub struct VirtIoHalImpl;

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

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        let vaddr = buffer.as_ptr() as *mut u8 as usize;
        vaddr - KERNEL_BASE
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Nothing to do, as the host already has access to all memory and we didn't copy the buffer
        // anywhere else.
    }
}
