#[cfg(not(feature = "io_buffer_pool"))]
use alloc::vec;
use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};

#[cfg(feature = "io_buffer_pool")]
use crate::mutex::SpinNoIrqLock;
#[cfg(feature = "io_buffer_pool")]
use core::sync::atomic::{AtomicUsize, Ordering};

/// The largest temporary buffer retained by the bounded per-hart cache.
#[cfg(feature = "io_buffer_pool")]
pub const MAX_CACHED_IO_BUFFER_SIZE: usize = crate::config::PAGE_SIZE * 16;

#[cfg(feature = "io_buffer_pool")]
static IO_BUFFER_POOL: [SpinNoIrqLock<Option<Vec<u8>>>; crate::arch::smp::MAX_HARTS] =
    [const { SpinNoIrqLock::new(None) }; crate::arch::smp::MAX_HARTS];
#[cfg(feature = "io_buffer_pool")]
static IO_BUFFER_DRAIN_EPOCH: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub enum IoBufferKind {
    Read,
    Pread,
    Pwrite,
    CopyFile,
    Write,
    Splice,
    Tee,
}

/// An initialized kernel bounce buffer with an optional bounded cache backend.
///
/// Cached bytes are intentionally not cleared on every checkout. Callers must
/// only expose bytes that their producer reports as initialized, exactly as
/// required by the existing `FileOp::read` and pipe read contracts. Newly
/// grown bytes are initialized by `Vec::resize`.
pub struct KernelIoBuffer {
    bytes: Vec<u8>,
    len: usize,
    #[cfg(feature = "io_buffer_pool")]
    cache_epoch: usize,
}

impl KernelIoBuffer {
    pub fn new(len: usize, kind: IoBufferKind) -> Self {
        crate::perf::io_buffer_acquire(1);
        crate::perf::io_buffer_requested_bytes(len);
        record_kind(kind);
        let started = crate::perf::now_ticks();

        #[cfg(feature = "io_buffer_pool")]
        let cache_epoch = IO_BUFFER_DRAIN_EPOCH.load(Ordering::Acquire);

        #[cfg(feature = "io_buffer_pool")]
        let mut bytes = {
            let hart = crate::arch::smp::current_hart_id();
            debug_assert!(hart < IO_BUFFER_POOL.len());
            match IO_BUFFER_POOL[hart].lock().take() {
                Some(bytes) => {
                    crate::perf::io_buffer_cache_hit(1);
                    bytes
                }
                None => {
                    crate::perf::io_buffer_cache_miss(1);
                    Vec::new()
                }
            }
        };

        #[cfg(not(feature = "io_buffer_pool"))]
        let bytes = {
            crate::perf::io_buffer_cache_miss(1);
            vec![0; len]
        };

        #[cfg(feature = "io_buffer_pool")]
        {
            let old_capacity = bytes.capacity();
            if bytes.len() < len {
                bytes.resize(len, 0);
            }
            if bytes.capacity() > old_capacity {
                crate::perf::io_buffer_grow(1);
                crate::perf::io_buffer_grow_bytes(bytes.capacity() - old_capacity);
            }
        }

        crate::perf::io_buffer_acquire_ticks(crate::perf::elapsed_since(started));
        Self {
            bytes,
            len,
            #[cfg(feature = "io_buffer_pool")]
            cache_epoch,
        }
    }
}

impl Deref for KernelIoBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes[..self.len]
    }
}

impl DerefMut for KernelIoBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.bytes[..self.len]
    }
}

impl Drop for KernelIoBuffer {
    fn drop(&mut self) {
        #[cfg(feature = "io_buffer_pool")]
        {
            if self.cache_epoch != IO_BUFFER_DRAIN_EPOCH.load(Ordering::Acquire) {
                crate::perf::io_buffer_dropped_release(1);
                return;
            }
            if self.bytes.capacity() > MAX_CACHED_IO_BUFFER_SIZE {
                crate::perf::io_buffer_dropped_release(1);
                return;
            }

            let hart = crate::arch::smp::current_hart_id();
            debug_assert!(hart < IO_BUFFER_POOL.len());
            let mut slot = IO_BUFFER_POOL[hart].lock();
            if slot.is_none() {
                *slot = Some(core::mem::take(&mut self.bytes));
                crate::perf::io_buffer_cached_release(1);
            } else {
                crate::perf::io_buffer_dropped_release(1);
            }
        }
    }
}

#[cfg(feature = "io_buffer_pool")]
pub fn drain_io_buffers() -> (usize, usize) {
    // Buffers currently checked out under the old epoch must not repopulate
    // the cache after this best-effort drain returns.
    IO_BUFFER_DRAIN_EPOCH.fetch_add(1, Ordering::AcqRel);
    let mut slots = 0usize;
    let mut bytes = 0usize;
    for slot in &IO_BUFFER_POOL {
        let buffer = slot.lock().take();
        if let Some(buffer) = buffer {
            slots += 1;
            bytes += buffer.capacity();
        }
    }
    (slots, bytes)
}

#[cfg(not(feature = "io_buffer_pool"))]
pub fn drain_io_buffers() -> (usize, usize) {
    (0, 0)
}

fn record_kind(kind: IoBufferKind) {
    match kind {
        IoBufferKind::Read => crate::perf::io_buffer_read_acquire(1),
        IoBufferKind::Pread => crate::perf::io_buffer_pread_acquire(1),
        IoBufferKind::Pwrite => crate::perf::io_buffer_pwrite_acquire(1),
        IoBufferKind::CopyFile => crate::perf::io_buffer_copy_file_acquire(1),
        IoBufferKind::Write => crate::perf::io_buffer_write_acquire(1),
        IoBufferKind::Splice => crate::perf::io_buffer_splice_acquire(1),
        IoBufferKind::Tee => crate::perf::io_buffer_tee_acquire(1),
    }
}
