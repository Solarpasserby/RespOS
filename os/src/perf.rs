//! Optional low-overhead counters for BuildStorm performance diagnosis.
//!
//! Call sites compile to no-ops unless the kernel is built with the
//! `perf_counters` feature.

use alloc::string::String;
#[cfg(feature = "perf_counters")]
use core::sync::atomic::{AtomicUsize, Ordering};

macro_rules! counters {
    ($($name:ident),+ $(,)?) => {
        #[cfg(feature = "perf_counters")]
        struct Counters {
            $($name: AtomicUsize,)+
        }

        #[cfg(feature = "perf_counters")]
        static COUNTERS: Counters = Counters {
            $($name: AtomicUsize::new(0),)+
        };

        #[derive(Clone, Copy, Default)]
        pub struct Snapshot {
            $(pub $name: usize,)+
        }

        pub fn snapshot() -> Snapshot {
            #[cfg(feature = "perf_counters")]
            {
                Snapshot {
                    $($name: COUNTERS.$name.load(Ordering::Relaxed),)+
                }
            }
            #[cfg(not(feature = "perf_counters"))]
            Snapshot::default()
        }

        pub fn reset() {
            #[cfg(feature = "perf_counters")]
            {
                $(COUNTERS.$name.store(0, Ordering::Relaxed);)+
                let heap_current = crate::mm::heap_allocated();
                COUNTERS.heap_current_bytes.store(heap_current, Ordering::Relaxed);
                COUNTERS.heap_peak_bytes.store(heap_current, Ordering::Relaxed);
                COUNTERS.dirty_pages_peak.store(
                    crate::fs::page_cache_dirty_page_count(),
                    Ordering::Relaxed,
                );
            }
        }
    };
}

counters!(
    file_closes,
    close_data_writebacks,
    explicit_fsyncs,
    filesystem_flushes,
    block_read_requests,
    block_read_bytes,
    block_write_requests,
    block_write_bytes,
    block_flushes,
    page_cache_hits,
    page_cache_misses,
    page_cache_evictions,
    page_cache_writeback_bytes,
    dirty_pages_peak,
    anonymous_faults,
    private_file_faults,
    shared_file_faults,
    cow_faults,
    context_switches,
    local_sfences,
    remote_rfences,
    scheduler_ipis,
    task_running_ticks,
    idle_ticks,
    ext4_lock_acquisitions,
    ext4_lock_wait_ticks,
    ext4_lock_hold_ticks,
    ext4_lock_max_wait_ticks,
    ext4_lock_max_hold_ticks,
    heap_alloc_calls,
    heap_dealloc_calls,
    heap_alloc_bytes,
    heap_dealloc_bytes,
    heap_current_bytes,
    heap_peak_bytes,
    heap_alloc_ticks,
    heap_dealloc_ticks,
    heap_max_alloc_ticks,
    heap_max_dealloc_ticks,
);

#[cfg(feature = "perf_counters")]
fn observe_max(counter: &AtomicUsize, value: usize) {
    let mut current = counter.load(Ordering::Relaxed);
    while value > current {
        match counter.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

macro_rules! increment_functions {
    ($(($fn_name:ident, $field:ident)),+ $(,)?) => {
        $(
            #[inline(always)]
            pub fn $fn_name(value: usize) {
                #[cfg(feature = "perf_counters")]
                COUNTERS.$field.fetch_add(value, Ordering::Relaxed);
                #[cfg(not(feature = "perf_counters"))]
                let _ = value;
            }
        )+
    };
}

increment_functions!(
    (file_close, file_closes),
    (close_data_writeback, close_data_writebacks),
    (explicit_fsync, explicit_fsyncs),
    (filesystem_flush, filesystem_flushes),
    (block_read_request, block_read_requests),
    (block_read_bytes, block_read_bytes),
    (block_write_request, block_write_requests),
    (block_write_bytes, block_write_bytes),
    (block_flush, block_flushes),
    (page_cache_hit, page_cache_hits),
    (page_cache_miss, page_cache_misses),
    (page_cache_eviction, page_cache_evictions),
    (page_cache_writeback_bytes, page_cache_writeback_bytes),
    (anonymous_fault, anonymous_faults),
    (private_file_fault, private_file_faults),
    (shared_file_fault, shared_file_faults),
    (cow_fault, cow_faults),
    (context_switch, context_switches),
    (local_sfence, local_sfences),
    (remote_rfence, remote_rfences),
    (scheduler_ipi, scheduler_ipis),
    (task_running_ticks, task_running_ticks),
    (idle_ticks, idle_ticks),
    (ext4_lock_acquisition, ext4_lock_acquisitions),
    (ext4_lock_wait_ticks, ext4_lock_wait_ticks),
    (ext4_lock_hold_ticks, ext4_lock_hold_ticks),
);

#[inline(always)]
pub fn observe_dirty_pages(value: usize) {
    #[cfg(feature = "perf_counters")]
    observe_max(&COUNTERS.dirty_pages_peak, value);
    #[cfg(not(feature = "perf_counters"))]
    let _ = value;
}

#[inline(always)]
pub fn observe_ext4_lock_wait(value: usize) {
    ext4_lock_acquisition(1);
    ext4_lock_wait_ticks(value);
    #[cfg(feature = "perf_counters")]
    observe_max(&COUNTERS.ext4_lock_max_wait_ticks, value);
}

#[inline(always)]
pub fn observe_ext4_lock_hold(value: usize) {
    ext4_lock_hold_ticks(value);
    #[cfg(feature = "perf_counters")]
    observe_max(&COUNTERS.ext4_lock_max_hold_ticks, value);
}

#[inline(always)]
pub fn heap_alloc(size: usize, ticks: usize, succeeded: bool) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS.heap_alloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .heap_alloc_ticks
            .fetch_add(ticks, Ordering::Relaxed);
        observe_max(&COUNTERS.heap_max_alloc_ticks, ticks);
        if succeeded {
            COUNTERS.heap_alloc_bytes.fetch_add(size, Ordering::Relaxed);
            let current = COUNTERS
                .heap_current_bytes
                .fetch_add(size, Ordering::Relaxed)
                + size;
            observe_max(&COUNTERS.heap_peak_bytes, current);
        }
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (size, ticks, succeeded);
}

#[inline(always)]
pub fn heap_dealloc(size: usize, ticks: usize) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS.heap_dealloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .heap_dealloc_bytes
            .fetch_add(size, Ordering::Relaxed);
        COUNTERS
            .heap_dealloc_ticks
            .fetch_add(ticks, Ordering::Relaxed);
        COUNTERS
            .heap_current_bytes
            .fetch_sub(size, Ordering::Relaxed);
        observe_max(&COUNTERS.heap_max_dealloc_ticks, ticks);
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (size, ticks);
}

pub fn render() -> String {
    use core::fmt::Write;

    if !cfg!(feature = "perf_counters") {
        return String::from("enabled=0\n");
    }
    let s = snapshot();
    let mut out = String::new();
    let _ = writeln!(out, "enabled=1");
    let _ = writeln!(
        out,
        "file_closes={} close_data_writebacks={} explicit_fsyncs={} filesystem_flushes={}",
        s.file_closes, s.close_data_writebacks, s.explicit_fsyncs, s.filesystem_flushes
    );
    let _ = writeln!(
        out,
        "block_read_requests={} block_read_bytes={} block_write_requests={} block_write_bytes={} block_flushes={}",
        s.block_read_requests,
        s.block_read_bytes,
        s.block_write_requests,
        s.block_write_bytes,
        s.block_flushes
    );
    let _ = writeln!(
        out,
        "page_cache_hits={} page_cache_misses={} page_cache_evictions={} page_cache_writeback_bytes={} dirty_pages_peak={}",
        s.page_cache_hits,
        s.page_cache_misses,
        s.page_cache_evictions,
        s.page_cache_writeback_bytes,
        s.dirty_pages_peak
    );
    let _ = writeln!(
        out,
        "page_cache_pages={} page_cache_dirty_pages={} page_cache_lru_entries={} page_cache_registry={} shared_file_page_entries={} free_frames={}",
        crate::fs::page_cache_page_count(),
        crate::fs::page_cache_dirty_page_count(),
        crate::fs::page_cache_lru_entry_count(),
        crate::fs::page_cache_registry_count(),
        crate::mm::shared_file_page_entry_count(),
        crate::mm::free_frame_count()
    );
    let _ = writeln!(
        out,
        "anonymous_faults={} private_file_faults={} shared_file_faults={} cow_faults={}",
        s.anonymous_faults, s.private_file_faults, s.shared_file_faults, s.cow_faults
    );
    let _ = writeln!(
        out,
        "context_switches={} local_sfences={} remote_rfences={} scheduler_ipis={} task_running_ticks={} idle_ticks={}",
        s.context_switches,
        s.local_sfences,
        s.remote_rfences,
        s.scheduler_ipis,
        s.task_running_ticks,
        s.idle_ticks
    );
    let _ = writeln!(
        out,
        "ext4_lock_acquisitions={} ext4_lock_wait_ticks={} ext4_lock_hold_ticks={} ext4_lock_max_wait_ticks={} ext4_lock_max_hold_ticks={}",
        s.ext4_lock_acquisitions,
        s.ext4_lock_wait_ticks,
        s.ext4_lock_hold_ticks,
        s.ext4_lock_max_wait_ticks,
        s.ext4_lock_max_hold_ticks
    );
    let _ = writeln!(
        out,
        "heap_alloc_calls={} heap_dealloc_calls={} heap_alloc_bytes={} heap_dealloc_bytes={} heap_current_bytes={} heap_peak_bytes={}",
        s.heap_alloc_calls,
        s.heap_dealloc_calls,
        s.heap_alloc_bytes,
        s.heap_dealloc_bytes,
        s.heap_current_bytes,
        s.heap_peak_bytes
    );
    let _ = writeln!(
        out,
        "heap_alloc_ticks={} heap_dealloc_ticks={} heap_max_alloc_ticks={} heap_max_dealloc_ticks={} clock_hz={}",
        s.heap_alloc_ticks,
        s.heap_dealloc_ticks,
        s.heap_max_alloc_ticks,
        s.heap_max_dealloc_ticks,
        crate::timer::get_hardware_clock_freq()
    );
    out
}

#[inline(always)]
pub fn now_ticks() -> usize {
    #[cfg(feature = "perf_counters")]
    {
        crate::timer::get_time()
    }
    #[cfg(not(feature = "perf_counters"))]
    {
        0
    }
}

#[inline(always)]
pub fn elapsed_since(start: usize) -> usize {
    now_ticks().wrapping_sub(start)
}
