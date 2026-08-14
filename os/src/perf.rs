//! Optional low-overhead counters for BuildStorm performance diagnosis.
//!
//! Call sites compile to no-ops unless the kernel is built with the
//! `perf_counters` feature.

use alloc::string::String;
#[cfg(feature = "perf_counters")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "perf_counters")]
static SCHEDULER_READY_CURRENT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "perf_counters")]
static RUNNING_HARTS_CURRENT: AtomicUsize = AtomicUsize::new(0);

const HEAP_CLASS_COUNT: usize = 10;
const HEAP_TIMING_SAMPLE_RATE: usize = 64;
const HEAP_CLASS_UPPER_BOUNDS: [usize; HEAP_CLASS_COUNT] =
    [16, 32, 64, 128, 256, 512, 1024, 2048, 4096, usize::MAX];

#[cfg(feature = "perf_counters")]
struct HeapClassCounters {
    alloc_calls: [AtomicUsize; HEAP_CLASS_COUNT],
    dealloc_calls: [AtomicUsize; HEAP_CLASS_COUNT],
    alloc_bytes: [AtomicUsize; HEAP_CLASS_COUNT],
    dealloc_bytes: [AtomicUsize; HEAP_CLASS_COUNT],
    alloc_wait_ticks: [AtomicUsize; HEAP_CLASS_COUNT],
    dealloc_wait_ticks: [AtomicUsize; HEAP_CLASS_COUNT],
    alloc_core_ticks: [AtomicUsize; HEAP_CLASS_COUNT],
    dealloc_core_ticks: [AtomicUsize; HEAP_CLASS_COUNT],
    alloc_ticks: AtomicUsize,
    dealloc_ticks: AtomicUsize,
    max_alloc_ticks: AtomicUsize,
    max_dealloc_ticks: AtomicUsize,
    high_alignment_allocs: AtomicUsize,
    alloc_timing_samples: AtomicUsize,
    dealloc_timing_samples: AtomicUsize,
    timing_sequence: AtomicUsize,
    magazine_hits: AtomicUsize,
    magazine_misses: AtomicUsize,
    magazine_cached_frees: AtomicUsize,
    magazine_refill_blocks: AtomicUsize,
    magazine_overflow_returns: AtomicUsize,
    magazine_reclaim_blocks: AtomicUsize,
}

#[cfg(feature = "perf_counters")]
static HEAP_CLASSES: [HeapClassCounters; crate::arch::smp::MAX_HARTS] = [const {
    HeapClassCounters {
        alloc_calls: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        dealloc_calls: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        alloc_bytes: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        dealloc_bytes: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        alloc_wait_ticks: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        dealloc_wait_ticks: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        alloc_core_ticks: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        dealloc_core_ticks: [const { AtomicUsize::new(0) }; HEAP_CLASS_COUNT],
        alloc_ticks: AtomicUsize::new(0),
        dealloc_ticks: AtomicUsize::new(0),
        max_alloc_ticks: AtomicUsize::new(0),
        max_dealloc_ticks: AtomicUsize::new(0),
        high_alignment_allocs: AtomicUsize::new(0),
        alloc_timing_samples: AtomicUsize::new(0),
        dealloc_timing_samples: AtomicUsize::new(0),
        timing_sequence: AtomicUsize::new(0),
        magazine_hits: AtomicUsize::new(0),
        magazine_misses: AtomicUsize::new(0),
        magazine_cached_frees: AtomicUsize::new(0),
        magazine_refill_blocks: AtomicUsize::new(0),
        magazine_overflow_returns: AtomicUsize::new(0),
        magazine_reclaim_blocks: AtomicUsize::new(0),
    }
};
    crate::arch::smp::MAX_HARTS];

#[derive(Clone, Copy, Default)]
struct HeapClassSnapshot {
    alloc_calls: [usize; HEAP_CLASS_COUNT],
    dealloc_calls: [usize; HEAP_CLASS_COUNT],
    alloc_bytes: [usize; HEAP_CLASS_COUNT],
    dealloc_bytes: [usize; HEAP_CLASS_COUNT],
    alloc_wait_ticks: [usize; HEAP_CLASS_COUNT],
    dealloc_wait_ticks: [usize; HEAP_CLASS_COUNT],
    alloc_core_ticks: [usize; HEAP_CLASS_COUNT],
    dealloc_core_ticks: [usize; HEAP_CLASS_COUNT],
    alloc_ticks: usize,
    dealloc_ticks: usize,
    max_alloc_ticks: usize,
    max_dealloc_ticks: usize,
    high_alignment_allocs: usize,
    alloc_timing_samples: usize,
    dealloc_timing_samples: usize,
    magazine_hits: usize,
    magazine_misses: usize,
    magazine_cached_frees: usize,
    magazine_refill_blocks: usize,
    magazine_overflow_returns: usize,
    magazine_reclaim_blocks: usize,
}

#[inline(always)]
#[cfg(feature = "perf_counters")]
fn heap_class_index(size: usize, align: usize) -> usize {
    let effective_size = size.max(align);
    HEAP_CLASS_UPPER_BOUNDS
        .iter()
        .position(|upper| effective_size <= *upper)
        .unwrap_or(HEAP_CLASS_COUNT - 1)
}

#[inline(always)]
#[cfg(feature = "perf_counters")]
fn current_heap_classes() -> &'static HeapClassCounters {
    let hart = crate::arch::smp::current_hart_id();
    debug_assert!(hart < HEAP_CLASSES.len());
    &HEAP_CLASSES[hart]
}

fn heap_class_snapshot() -> HeapClassSnapshot {
    #[cfg(feature = "perf_counters")]
    {
        let mut snapshot = HeapClassSnapshot {
            alloc_calls: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.alloc_calls[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            dealloc_calls: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.dealloc_calls[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            alloc_bytes: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.alloc_bytes[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            dealloc_bytes: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.dealloc_bytes[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            alloc_wait_ticks: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.alloc_wait_ticks[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            dealloc_wait_ticks: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.dealloc_wait_ticks[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            alloc_core_ticks: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.alloc_core_ticks[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            dealloc_core_ticks: core::array::from_fn(|idx| {
                HEAP_CLASSES
                    .iter()
                    .map(|classes| classes.dealloc_core_ticks[idx].load(Ordering::Relaxed))
                    .sum()
            }),
            ..HeapClassSnapshot::default()
        };
        for classes in &HEAP_CLASSES {
            snapshot.alloc_ticks += classes.alloc_ticks.load(Ordering::Relaxed);
            snapshot.dealloc_ticks += classes.dealloc_ticks.load(Ordering::Relaxed);
            snapshot.max_alloc_ticks = snapshot
                .max_alloc_ticks
                .max(classes.max_alloc_ticks.load(Ordering::Relaxed));
            snapshot.max_dealloc_ticks = snapshot
                .max_dealloc_ticks
                .max(classes.max_dealloc_ticks.load(Ordering::Relaxed));
            snapshot.high_alignment_allocs += classes.high_alignment_allocs.load(Ordering::Relaxed);
            snapshot.alloc_timing_samples += classes.alloc_timing_samples.load(Ordering::Relaxed);
            snapshot.dealloc_timing_samples +=
                classes.dealloc_timing_samples.load(Ordering::Relaxed);
            snapshot.magazine_hits += classes.magazine_hits.load(Ordering::Relaxed);
            snapshot.magazine_misses += classes.magazine_misses.load(Ordering::Relaxed);
            snapshot.magazine_cached_frees += classes.magazine_cached_frees.load(Ordering::Relaxed);
            snapshot.magazine_refill_blocks +=
                classes.magazine_refill_blocks.load(Ordering::Relaxed);
            snapshot.magazine_overflow_returns +=
                classes.magazine_overflow_returns.load(Ordering::Relaxed);
            snapshot.magazine_reclaim_blocks +=
                classes.magazine_reclaim_blocks.load(Ordering::Relaxed);
        }
        snapshot
    }
    #[cfg(not(feature = "perf_counters"))]
    HeapClassSnapshot::default()
}

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
                crate::mm::reset_heap_perf_peak();
                COUNTERS.dirty_pages_peak.store(
                    crate::fs::page_cache_dirty_page_count(),
                    Ordering::Relaxed,
                );
                for classes in &HEAP_CLASSES {
                    for idx in 0..HEAP_CLASS_COUNT {
                        classes.alloc_calls[idx].store(0, Ordering::Relaxed);
                        classes.dealloc_calls[idx].store(0, Ordering::Relaxed);
                        classes.alloc_bytes[idx].store(0, Ordering::Relaxed);
                        classes.dealloc_bytes[idx].store(0, Ordering::Relaxed);
                        classes.alloc_wait_ticks[idx].store(0, Ordering::Relaxed);
                        classes.dealloc_wait_ticks[idx].store(0, Ordering::Relaxed);
                        classes.alloc_core_ticks[idx].store(0, Ordering::Relaxed);
                        classes.dealloc_core_ticks[idx].store(0, Ordering::Relaxed);
                    }
                    classes.alloc_ticks.store(0, Ordering::Relaxed);
                    classes.dealloc_ticks.store(0, Ordering::Relaxed);
                    classes.max_alloc_ticks.store(0, Ordering::Relaxed);
                    classes.max_dealloc_ticks.store(0, Ordering::Relaxed);
                    classes.high_alignment_allocs.store(0, Ordering::Relaxed);
                    classes.alloc_timing_samples.store(0, Ordering::Relaxed);
                    classes.dealloc_timing_samples.store(0, Ordering::Relaxed);
                    classes.timing_sequence.store(0, Ordering::Relaxed);
                    classes.magazine_hits.store(0, Ordering::Relaxed);
                    classes.magazine_misses.store(0, Ordering::Relaxed);
                    classes.magazine_cached_frees.store(0, Ordering::Relaxed);
                    classes.magazine_refill_blocks.store(0, Ordering::Relaxed);
                    classes.magazine_overflow_returns.store(0, Ordering::Relaxed);
                    classes.magazine_reclaim_blocks.store(0, Ordering::Relaxed);
                }
                crate::drivers::reset_bounce_perf();
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
    block_read_ticks,
    block_read_512_or_less,
    block_read_4k_or_less,
    block_read_64k_or_less,
    block_read_over_64k,
    block_write_requests,
    block_write_bytes,
    block_write_ticks,
    block_flushes,
    page_cache_hits,
    page_cache_misses,
    page_cache_evictions,
    page_cache_writeback_bytes,
    page_cache_fill_calls,
    page_cache_fill_bytes,
    page_cache_fill_candidate_pages,
    page_cache_fill_published_pages,
    page_cache_fill_raced_pages,
    inode_read_calls,
    inode_read_requested_bytes,
    inode_read_completed_bytes,
    inode_read_ticks,
    ext4_stat_calls,
    ext4_stat_ticks,
    ext4_stat_cache_hits,
    ext4_stat_cache_misses,
    ext4_stat_cache_refills,
    ext4_stat_cache_uncacheable,
    ext4_stat_cache_invalidations,
    ext4_lookup_calls,
    ext4_lookup_ticks,
    ext4_readdir_calls,
    ext4_readdir_ticks,
    ext4_readdir_dirent_type_known,
    ext4_readdir_dirent_type_unknown,
    ext4_create_calls,
    ext4_create_ticks,
    ext4_write_calls,
    ext4_write_ticks,
    ext4_set_times_calls,
    ext4_set_times_atime_updates,
    ext4_set_times_mtime_updates,
    ext4_set_mode_calls,
    ext4_set_owner_calls,
    dentry_cache_hits,
    dentry_cache_misses,
    dentry_cache_evictions,
    dirty_pages_peak,
    anonymous_faults,
    private_file_faults,
    shared_file_faults,
    cow_faults,
    context_switches,
    local_sfences,
    local_sfence_ticks,
    local_sfence_max_ticks,
    remote_rfences,
    remote_rfence_target_harts,
    remote_rfence_empty_requests,
    remote_rfence_wait_ticks,
    remote_rfence_max_wait_ticks,
    full_tlb_invalidations,
    asid_tlb_invalidations,
    tlb_shootdown_all_requests,
    tlb_shootdown_address_space_requests,
    tlb_shootdown_range_requests,
    tlb_shootdown_range_pages,
    tlb_shootdown_range_max_pages,
    tlb_shootdown_range_single_page,
    tlb_shootdown_range_le16_pages,
    tlb_shootdown_range_le256_pages,
    tlb_shootdown_range_gt256_pages,
    tlb_shootdown_invalid_requests,
    tlb_flush_calls,
    tlb_fresh_map_flushes,
    tlb_cow_flushes,
    tlb_flush_retired_batches,
    tlb_flush_retired_frames,
    scheduler_ipis,
    ipis_received,
    scheduler_lock_acquisitions,
    scheduler_lock_wait_ticks,
    scheduler_lock_max_wait_ticks,
    scheduler_ready_peak,
    concurrency_samples,
    running_harts_0,
    running_harts_1,
    running_harts_2_3,
    running_harts_4_7,
    running_harts_8_plus,
    scheduler_ready_0,
    scheduler_ready_1,
    scheduler_ready_2_3,
    scheduler_ready_4_7,
    scheduler_ready_8_plus,
    scheduler_yields,
    syscall_yields,
    quiescence_yields,
    fs_yields,
    stdio_yields,
    tty_yields,
    pipe_yields,
    fs_syscall_yields,
    futex_yields,
    net_yields,
    unix_socket_yields,
    tcp_wait_yields,
    tcp_connect_yields,
    udp_wait_yields,
    process_yields,
    signal_time_yields,
    special_fd_yields,
    timer_preemptions,
    blocking_switches,
    task_running_ticks,
    idle_ticks,
    user_traps,
    user_syscall_traps,
    user_page_fault_traps,
    user_timer_traps,
    user_ipi_traps,
    extension_state_eager_saves,
    ext4_lock_acquisitions,
    ext4_lock_wait_ticks,
    ext4_lock_hold_ticks,
    ext4_lock_max_wait_ticks,
    ext4_lock_max_hold_ticks,
    ext4_lock_stat_acquisitions,
    ext4_lock_stat_wait_ticks,
    ext4_lock_stat_hold_ticks,
    ext4_lock_lookup_acquisitions,
    ext4_lock_lookup_wait_ticks,
    ext4_lock_lookup_hold_ticks,
    ext4_lock_read_acquisitions,
    ext4_lock_read_wait_ticks,
    ext4_lock_read_hold_ticks,
    ext4_lock_write_acquisitions,
    ext4_lock_write_wait_ticks,
    ext4_lock_write_hold_ticks,
    ext4_lock_readdir_acquisitions,
    ext4_lock_readdir_wait_ticks,
    ext4_lock_readdir_hold_ticks,
    ext4_lock_namespace_acquisitions,
    ext4_lock_namespace_wait_ticks,
    ext4_lock_namespace_hold_ticks,
    ext4_lock_attributes_acquisitions,
    ext4_lock_attributes_wait_ticks,
    ext4_lock_attributes_hold_ticks,
    ext4_lock_superblock_acquisitions,
    ext4_lock_superblock_wait_ticks,
    ext4_lock_superblock_hold_ticks,
    ext4_lower_calls,
    ext4_lower_ticks,
    ext4_lower_stat_calls,
    ext4_lower_stat_ticks,
    ext4_lower_lookup_calls,
    ext4_lower_lookup_ticks,
    ext4_lower_read_calls,
    ext4_lower_read_ticks,
    ext4_lower_write_calls,
    ext4_lower_write_ticks,
    ext4_lower_readdir_calls,
    ext4_lower_readdir_ticks,
    ext4_lower_namespace_calls,
    ext4_lower_namespace_ticks,
    ext4_lower_attributes_calls,
    ext4_lower_attributes_ticks,
    ext4_lower_superblock_calls,
    ext4_lower_superblock_ticks,
    frame_alloc_calls,
    frame_alloc_failures,
    frame_alloc_ticks,
    frame_alloc_lock_wait_ticks,
    frame_alloc_core_ticks,
    frame_alloc_clear_ticks,
    frame_alloc_max_ticks,
    frame_dealloc_calls,
    frame_dealloc_ticks,
    frame_dealloc_lock_wait_ticks,
    frame_dealloc_core_ticks,
    frame_dealloc_max_ticks,
    copy_from_user_calls,
    copy_from_user_bytes,
    copy_from_user_ticks,
    copy_to_user_calls,
    copy_to_user_bytes,
    copy_to_user_ticks,
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
    (block_read_ticks, block_read_ticks),
    (block_write_request, block_write_requests),
    (block_write_bytes, block_write_bytes),
    (block_write_ticks, block_write_ticks),
    (block_flush, block_flushes),
    (page_cache_hit, page_cache_hits),
    (page_cache_miss, page_cache_misses),
    (page_cache_eviction, page_cache_evictions),
    (page_cache_writeback_bytes, page_cache_writeback_bytes),
    (page_cache_fill_call, page_cache_fill_calls),
    (page_cache_fill_bytes, page_cache_fill_bytes),
    (
        page_cache_fill_candidate_pages,
        page_cache_fill_candidate_pages
    ),
    (
        page_cache_fill_published_pages,
        page_cache_fill_published_pages
    ),
    (page_cache_fill_raced_pages, page_cache_fill_raced_pages),
    (inode_read_call, inode_read_calls),
    (inode_read_requested_bytes, inode_read_requested_bytes),
    (inode_read_completed_bytes, inode_read_completed_bytes),
    (inode_read_ticks, inode_read_ticks),
    (ext4_stat_call, ext4_stat_calls),
    (ext4_stat_ticks, ext4_stat_ticks),
    (ext4_stat_cache_hit, ext4_stat_cache_hits),
    (ext4_stat_cache_miss, ext4_stat_cache_misses),
    (ext4_stat_cache_refill, ext4_stat_cache_refills),
    (ext4_stat_cache_uncacheable, ext4_stat_cache_uncacheable),
    (ext4_stat_cache_invalidation, ext4_stat_cache_invalidations),
    (ext4_lookup_call, ext4_lookup_calls),
    (ext4_lookup_ticks, ext4_lookup_ticks),
    (ext4_readdir_call, ext4_readdir_calls),
    (ext4_readdir_ticks, ext4_readdir_ticks),
    (
        ext4_readdir_dirent_type_known,
        ext4_readdir_dirent_type_known
    ),
    (
        ext4_readdir_dirent_type_unknown,
        ext4_readdir_dirent_type_unknown
    ),
    (ext4_create_call, ext4_create_calls),
    (ext4_create_ticks, ext4_create_ticks),
    (ext4_write_call, ext4_write_calls),
    (ext4_write_ticks, ext4_write_ticks),
    (ext4_set_times_call, ext4_set_times_calls),
    (ext4_set_times_atime_update, ext4_set_times_atime_updates),
    (ext4_set_times_mtime_update, ext4_set_times_mtime_updates),
    (ext4_set_mode_call, ext4_set_mode_calls),
    (ext4_set_owner_call, ext4_set_owner_calls),
    (dentry_cache_hit, dentry_cache_hits),
    (dentry_cache_miss, dentry_cache_misses),
    (dentry_cache_eviction, dentry_cache_evictions),
    (anonymous_fault, anonymous_faults),
    (private_file_fault, private_file_faults),
    (shared_file_fault, shared_file_faults),
    (cow_fault, cow_faults),
    (context_switch, context_switches),
    (local_sfence, local_sfences),
    (local_sfence_ticks, local_sfence_ticks),
    (remote_rfence_empty_request, remote_rfence_empty_requests),
    (full_tlb_invalidation, full_tlb_invalidations),
    (asid_tlb_invalidation, asid_tlb_invalidations),
    (tlb_shootdown_all_request, tlb_shootdown_all_requests),
    (
        tlb_shootdown_address_space_request,
        tlb_shootdown_address_space_requests
    ),
    (tlb_shootdown_range_request, tlb_shootdown_range_requests),
    (tlb_shootdown_range_pages, tlb_shootdown_range_pages),
    (
        tlb_shootdown_invalid_request,
        tlb_shootdown_invalid_requests
    ),
    (tlb_flush_call, tlb_flush_calls),
    (tlb_fresh_map_flush, tlb_fresh_map_flushes),
    (tlb_cow_flush, tlb_cow_flushes),
    (tlb_flush_retired_batch, tlb_flush_retired_batches),
    (tlb_flush_retired_frames, tlb_flush_retired_frames),
    (scheduler_ipi, scheduler_ipis),
    (ipi_received, ipis_received),
    (scheduler_lock_acquisition, scheduler_lock_acquisitions),
    (scheduler_lock_wait_ticks, scheduler_lock_wait_ticks),
    (scheduler_yield, scheduler_yields),
    (syscall_yield, syscall_yields),
    (quiescence_yield, quiescence_yields),
    (fs_yield, fs_yields),
    (stdio_yield, stdio_yields),
    (tty_yield, tty_yields),
    (pipe_yield, pipe_yields),
    (fs_syscall_yield, fs_syscall_yields),
    (futex_yield, futex_yields),
    (net_yield, net_yields),
    (unix_socket_yield, unix_socket_yields),
    (tcp_wait_yield, tcp_wait_yields),
    (tcp_connect_yield, tcp_connect_yields),
    (udp_wait_yield, udp_wait_yields),
    (process_yield, process_yields),
    (signal_time_yield, signal_time_yields),
    (special_fd_yield, special_fd_yields),
    (timer_preemption, timer_preemptions),
    (blocking_switch, blocking_switches),
    (task_running_ticks, task_running_ticks),
    (idle_ticks, idle_ticks),
    (user_trap, user_traps),
    (user_syscall_trap, user_syscall_traps),
    (user_page_fault_trap, user_page_fault_traps),
    (user_timer_trap, user_timer_traps),
    (user_ipi_trap, user_ipi_traps),
    (extension_state_eager_save, extension_state_eager_saves),
    (ext4_lock_acquisition, ext4_lock_acquisitions),
    (ext4_lock_wait_ticks, ext4_lock_wait_ticks),
    (ext4_lock_hold_ticks, ext4_lock_hold_ticks),
    (ext4_lock_stat_acquisition, ext4_lock_stat_acquisitions),
    (ext4_lock_stat_wait_ticks, ext4_lock_stat_wait_ticks),
    (ext4_lock_stat_hold_ticks, ext4_lock_stat_hold_ticks),
    (ext4_lock_lookup_acquisition, ext4_lock_lookup_acquisitions),
    (ext4_lock_lookup_wait_ticks, ext4_lock_lookup_wait_ticks),
    (ext4_lock_lookup_hold_ticks, ext4_lock_lookup_hold_ticks),
    (ext4_lock_read_acquisition, ext4_lock_read_acquisitions),
    (ext4_lock_read_wait_ticks, ext4_lock_read_wait_ticks),
    (ext4_lock_read_hold_ticks, ext4_lock_read_hold_ticks),
    (ext4_lock_write_acquisition, ext4_lock_write_acquisitions),
    (ext4_lock_write_wait_ticks, ext4_lock_write_wait_ticks),
    (ext4_lock_write_hold_ticks, ext4_lock_write_hold_ticks),
    (
        ext4_lock_readdir_acquisition,
        ext4_lock_readdir_acquisitions
    ),
    (ext4_lock_readdir_wait_ticks, ext4_lock_readdir_wait_ticks),
    (ext4_lock_readdir_hold_ticks, ext4_lock_readdir_hold_ticks),
    (
        ext4_lock_namespace_acquisition,
        ext4_lock_namespace_acquisitions
    ),
    (
        ext4_lock_namespace_wait_ticks,
        ext4_lock_namespace_wait_ticks
    ),
    (
        ext4_lock_namespace_hold_ticks,
        ext4_lock_namespace_hold_ticks
    ),
    (
        ext4_lock_attributes_acquisition,
        ext4_lock_attributes_acquisitions
    ),
    (
        ext4_lock_attributes_wait_ticks,
        ext4_lock_attributes_wait_ticks
    ),
    (
        ext4_lock_attributes_hold_ticks,
        ext4_lock_attributes_hold_ticks
    ),
    (
        ext4_lock_superblock_acquisition,
        ext4_lock_superblock_acquisitions
    ),
    (
        ext4_lock_superblock_wait_ticks,
        ext4_lock_superblock_wait_ticks
    ),
    (
        ext4_lock_superblock_hold_ticks,
        ext4_lock_superblock_hold_ticks
    ),
    (ext4_lower_call, ext4_lower_calls),
    (ext4_lower_ticks, ext4_lower_ticks),
    (ext4_lower_stat_call, ext4_lower_stat_calls),
    (ext4_lower_stat_ticks, ext4_lower_stat_ticks),
    (ext4_lower_lookup_call, ext4_lower_lookup_calls),
    (ext4_lower_lookup_ticks, ext4_lower_lookup_ticks),
    (ext4_lower_read_call, ext4_lower_read_calls),
    (ext4_lower_read_ticks, ext4_lower_read_ticks),
    (ext4_lower_write_call, ext4_lower_write_calls),
    (ext4_lower_write_ticks, ext4_lower_write_ticks),
    (ext4_lower_readdir_call, ext4_lower_readdir_calls),
    (ext4_lower_readdir_ticks, ext4_lower_readdir_ticks),
    (ext4_lower_namespace_call, ext4_lower_namespace_calls),
    (ext4_lower_namespace_ticks, ext4_lower_namespace_ticks),
    (ext4_lower_attributes_call, ext4_lower_attributes_calls),
    (ext4_lower_attributes_ticks, ext4_lower_attributes_ticks),
    (ext4_lower_superblock_call, ext4_lower_superblock_calls),
    (ext4_lower_superblock_ticks, ext4_lower_superblock_ticks),
);

#[inline(always)]
pub fn observe_tlb_shootdown_range_pages(value: usize) {
    #[cfg(feature = "perf_counters")]
    observe_max(&COUNTERS.tlb_shootdown_range_max_pages, value);
    #[cfg(not(feature = "perf_counters"))]
    let _ = value;
}

#[inline(always)]
pub fn classify_tlb_shootdown_range(pages: usize) {
    #[cfg(feature = "perf_counters")]
    let counter = if pages == 1 {
        &COUNTERS.tlb_shootdown_range_single_page
    } else if pages <= 16 {
        &COUNTERS.tlb_shootdown_range_le16_pages
    } else if pages <= 256 {
        &COUNTERS.tlb_shootdown_range_le256_pages
    } else {
        &COUNTERS.tlb_shootdown_range_gt256_pages
    };
    #[cfg(feature = "perf_counters")]
    counter.fetch_add(1, Ordering::Relaxed);
    #[cfg(not(feature = "perf_counters"))]
    let _ = pages;
}

#[inline(always)]
pub fn block_read_size(size: usize) {
    #[cfg(feature = "perf_counters")]
    let counter = if size <= 512 {
        &COUNTERS.block_read_512_or_less
    } else if size <= 4096 {
        &COUNTERS.block_read_4k_or_less
    } else if size <= 65536 {
        &COUNTERS.block_read_64k_or_less
    } else {
        &COUNTERS.block_read_over_64k
    };
    #[cfg(feature = "perf_counters")]
    counter.fetch_add(1, Ordering::Relaxed);
    #[cfg(not(feature = "perf_counters"))]
    let _ = size;
}

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
pub fn observe_scheduler_lock_wait(value: usize) {
    scheduler_lock_acquisition(1);
    scheduler_lock_wait_ticks(value);
    #[cfg(feature = "perf_counters")]
    observe_max(&COUNTERS.scheduler_lock_max_wait_ticks, value);
}

#[inline(always)]
pub fn observe_remote_rfence(target_harts: usize, wait_ticks: usize) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS.remote_rfences.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .remote_rfence_target_harts
            .fetch_add(target_harts, Ordering::Relaxed);
        COUNTERS
            .remote_rfence_wait_ticks
            .fetch_add(wait_ticks, Ordering::Relaxed);
        observe_max(&COUNTERS.remote_rfence_max_wait_ticks, wait_ticks);
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (target_harts, wait_ticks);
}

#[inline(always)]
pub fn observe_local_sfence(ticks: usize) {
    local_sfence(1);
    local_sfence_ticks(ticks);
    #[cfg(feature = "perf_counters")]
    observe_max(&COUNTERS.local_sfence_max_ticks, ticks);
}

#[inline(always)]
pub fn observe_scheduler_ready(value: usize) {
    #[cfg(feature = "perf_counters")]
    {
        SCHEDULER_READY_CURRENT.store(value, Ordering::Relaxed);
        observe_max(&COUNTERS.scheduler_ready_peak, value);
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = value;
}

/// Mark one hart as executing a scheduled task. This is deliberately separate
/// from scheduler locking so timer-side sampling stays lock-free.
#[inline(always)]
pub fn task_running_begin() {
    #[cfg(feature = "perf_counters")]
    {
        RUNNING_HARTS_CURRENT.fetch_add(1, Ordering::Relaxed);
    }
}

#[inline(always)]
pub fn task_running_end() {
    #[cfg(feature = "perf_counters")]
    {
        RUNNING_HARTS_CURRENT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Take one low-overhead concurrency sample from a timer interrupt.
#[inline(always)]
pub fn sample_concurrency() {
    #[cfg(feature = "perf_counters")]
    {
        fn bucket(
            value: usize,
            zero: &AtomicUsize,
            one: &AtomicUsize,
            two_three: &AtomicUsize,
            four_seven: &AtomicUsize,
            eight_plus: &AtomicUsize,
        ) {
            let target = match value {
                0 => zero,
                1 => one,
                2..=3 => two_three,
                4..=7 => four_seven,
                _ => eight_plus,
            };
            target.fetch_add(1, Ordering::Relaxed);
        }

        COUNTERS.concurrency_samples.fetch_add(1, Ordering::Relaxed);
        bucket(
            RUNNING_HARTS_CURRENT.load(Ordering::Relaxed),
            &COUNTERS.running_harts_0,
            &COUNTERS.running_harts_1,
            &COUNTERS.running_harts_2_3,
            &COUNTERS.running_harts_4_7,
            &COUNTERS.running_harts_8_plus,
        );
        bucket(
            SCHEDULER_READY_CURRENT.load(Ordering::Relaxed),
            &COUNTERS.scheduler_ready_0,
            &COUNTERS.scheduler_ready_1,
            &COUNTERS.scheduler_ready_2_3,
            &COUNTERS.scheduler_ready_4_7,
            &COUNTERS.scheduler_ready_8_plus,
        );
    }
}

#[inline(always)]
pub fn heap_alloc(
    size: usize,
    align: usize,
    ticks: usize,
    lock_wait_ticks: usize,
    core_ticks: usize,
    succeeded: bool,
    timing_sampled: bool,
) {
    #[cfg(feature = "perf_counters")]
    {
        let class = heap_class_index(size, align);
        let classes = current_heap_classes();
        classes.alloc_calls[class].fetch_add(1, Ordering::Relaxed);
        if timing_sampled {
            let estimated_ticks = ticks.saturating_mul(HEAP_TIMING_SAMPLE_RATE);
            let estimated_wait = lock_wait_ticks.saturating_mul(HEAP_TIMING_SAMPLE_RATE);
            let estimated_core = core_ticks.saturating_mul(HEAP_TIMING_SAMPLE_RATE);
            classes
                .alloc_ticks
                .fetch_add(estimated_ticks, Ordering::Relaxed);
            classes.alloc_wait_ticks[class].fetch_add(estimated_wait, Ordering::Relaxed);
            classes.alloc_core_ticks[class].fetch_add(estimated_core, Ordering::Relaxed);
            classes.alloc_timing_samples.fetch_add(1, Ordering::Relaxed);
            observe_max(&classes.max_alloc_ticks, ticks);
        }
        if align > size {
            classes
                .high_alignment_allocs
                .fetch_add(1, Ordering::Relaxed);
        }
        if succeeded {
            classes.alloc_bytes[class].fetch_add(size, Ordering::Relaxed);
        }
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (
        size,
        align,
        ticks,
        lock_wait_ticks,
        core_ticks,
        succeeded,
        timing_sampled,
    );
}

#[inline(always)]
pub fn heap_dealloc(
    size: usize,
    align: usize,
    ticks: usize,
    lock_wait_ticks: usize,
    core_ticks: usize,
    timing_sampled: bool,
) {
    #[cfg(feature = "perf_counters")]
    {
        let class = heap_class_index(size, align);
        let classes = current_heap_classes();
        classes.dealloc_calls[class].fetch_add(1, Ordering::Relaxed);
        classes.dealloc_bytes[class].fetch_add(size, Ordering::Relaxed);
        if timing_sampled {
            let estimated_ticks = ticks.saturating_mul(HEAP_TIMING_SAMPLE_RATE);
            let estimated_wait = lock_wait_ticks.saturating_mul(HEAP_TIMING_SAMPLE_RATE);
            let estimated_core = core_ticks.saturating_mul(HEAP_TIMING_SAMPLE_RATE);
            classes
                .dealloc_ticks
                .fetch_add(estimated_ticks, Ordering::Relaxed);
            classes.dealloc_wait_ticks[class].fetch_add(estimated_wait, Ordering::Relaxed);
            classes.dealloc_core_ticks[class].fetch_add(estimated_core, Ordering::Relaxed);
            classes
                .dealloc_timing_samples
                .fetch_add(1, Ordering::Relaxed);
            observe_max(&classes.max_dealloc_ticks, ticks);
        }
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (
        size,
        align,
        ticks,
        lock_wait_ticks,
        core_ticks,
        timing_sampled,
    );
}

#[inline(always)]
pub fn frame_alloc(
    ticks: usize,
    lock_wait_ticks: usize,
    core_ticks: usize,
    clear_ticks: usize,
    succeeded: bool,
) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS.frame_alloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .frame_alloc_ticks
            .fetch_add(ticks, Ordering::Relaxed);
        COUNTERS
            .frame_alloc_lock_wait_ticks
            .fetch_add(lock_wait_ticks, Ordering::Relaxed);
        COUNTERS
            .frame_alloc_core_ticks
            .fetch_add(core_ticks, Ordering::Relaxed);
        COUNTERS
            .frame_alloc_clear_ticks
            .fetch_add(clear_ticks, Ordering::Relaxed);
        observe_max(&COUNTERS.frame_alloc_max_ticks, ticks);
        if !succeeded {
            COUNTERS
                .frame_alloc_failures
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (ticks, lock_wait_ticks, core_ticks, clear_ticks, succeeded);
}

#[inline(always)]
pub fn frame_dealloc(ticks: usize, lock_wait_ticks: usize, core_ticks: usize) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS.frame_dealloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .frame_dealloc_ticks
            .fetch_add(ticks, Ordering::Relaxed);
        COUNTERS
            .frame_dealloc_lock_wait_ticks
            .fetch_add(lock_wait_ticks, Ordering::Relaxed);
        COUNTERS
            .frame_dealloc_core_ticks
            .fetch_add(core_ticks, Ordering::Relaxed);
        observe_max(&COUNTERS.frame_dealloc_max_ticks, ticks);
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (ticks, lock_wait_ticks, core_ticks);
}

#[inline(always)]
pub fn copy_from_user(bytes: usize, ticks: usize) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS
            .copy_from_user_calls
            .fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .copy_from_user_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        COUNTERS
            .copy_from_user_ticks
            .fetch_add(ticks, Ordering::Relaxed);
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (bytes, ticks);
}

#[inline(always)]
pub fn copy_to_user(bytes: usize, ticks: usize) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS.copy_to_user_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .copy_to_user_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        COUNTERS
            .copy_to_user_ticks
            .fetch_add(ticks, Ordering::Relaxed);
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (bytes, ticks);
}

pub fn render() -> String {
    use core::fmt::Write;

    if !cfg!(feature = "perf_counters") {
        return String::from("enabled=0\n");
    }
    // Capture class counters before constructing the output String: formatting
    // itself allocates and must not become part of the reported workload.
    let heap_classes = heap_class_snapshot();
    let (heap_current_bytes, heap_peak_bytes, heap_peak_exact) = {
        #[cfg(feature = "perf_counters")]
        {
            crate::mm::heap_perf_usage()
        }
        #[cfg(not(feature = "perf_counters"))]
        {
            (0, 0, true)
        }
    };
    let (magazine_cached_bytes, magazine_cached_peak_bytes) = {
        #[cfg(feature = "heap_magazine")]
        {
            crate::mm::heap_magazine_usage()
        }
        #[cfg(not(feature = "heap_magazine"))]
        {
            (0, 0)
        }
    };
    let s = snapshot();
    let bounce = crate::drivers::snapshot_bounce_perf();
    let class_alloc_calls: usize = heap_classes.alloc_calls.iter().sum();
    let class_dealloc_calls: usize = heap_classes.dealloc_calls.iter().sum();
    let class_alloc_bytes: usize = heap_classes.alloc_bytes.iter().sum();
    let class_dealloc_bytes: usize = heap_classes.dealloc_bytes.iter().sum();
    let class_alloc_wait_ticks: usize = heap_classes.alloc_wait_ticks.iter().sum();
    let class_dealloc_wait_ticks: usize = heap_classes.dealloc_wait_ticks.iter().sum();
    let class_alloc_core_ticks: usize = heap_classes.alloc_core_ticks.iter().sum();
    let class_dealloc_core_ticks: usize = heap_classes.dealloc_core_ticks.iter().sum();
    let mut out = String::new();
    let _ = writeln!(out, "enabled=1");
    let _ = writeln!(
        out,
        "file_closes={} close_data_writebacks={} explicit_fsyncs={} filesystem_flushes={}",
        s.file_closes, s.close_data_writebacks, s.explicit_fsyncs, s.filesystem_flushes
    );
    let _ = writeln!(
        out,
        "block_read_requests={} block_read_bytes={} block_read_ticks={} block_write_requests={} block_write_bytes={} block_write_ticks={} block_flushes={}",
        s.block_read_requests,
        s.block_read_bytes,
        s.block_read_ticks,
        s.block_write_requests,
        s.block_write_bytes,
        s.block_write_ticks,
        s.block_flushes
    );
    let _ = writeln!(
        out,
        "block_read_sizes_le512={} le4k={} le64k={} gt64k={}",
        s.block_read_512_or_less,
        s.block_read_4k_or_less,
        s.block_read_64k_or_less,
        s.block_read_over_64k
    );
    let _ = writeln!(
        out,
        "virtio_bounce_calls={} bytes={} copy_to_device_bytes={} copy_from_device_bytes={} allocations={} cache_hits={} share_ticks={} unshare_ticks={} active_peak={} active={} cached_buffers={} cached_bytes={}",
        bounce.calls,
        bounce.bytes,
        bounce.copy_to_device_bytes,
        bounce.copy_from_device_bytes,
        bounce.allocations,
        bounce.cache_hits,
        bounce.share_ticks,
        bounce.unshare_ticks,
        bounce.active_peak,
        bounce.active,
        bounce.cached_buffers,
        bounce.cached_bytes
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
        "page_cache_fill_calls={} page_cache_fill_bytes={} inode_read_calls={} inode_read_requested_bytes={} inode_read_completed_bytes={} inode_read_ticks={}",
        s.page_cache_fill_calls,
        s.page_cache_fill_bytes,
        s.inode_read_calls,
        s.inode_read_requested_bytes,
        s.inode_read_completed_bytes,
        s.inode_read_ticks
    );
    let _ = writeln!(
        out,
        "page_cache_fill_candidate_pages={} published_pages={} raced_pages={}",
        s.page_cache_fill_candidate_pages,
        s.page_cache_fill_published_pages,
        s.page_cache_fill_raced_pages
    );
    let _ = writeln!(
        out,
        "page_cache_pages={} page_cache_dirty_pages={} page_cache_lru_entries={} page_cache_registry={} dirty_owners={} shared_file_page_entries={} free_frames={}",
        crate::fs::page_cache_page_count(),
        crate::fs::page_cache_dirty_page_count(),
        crate::fs::page_cache_lru_entry_count(),
        crate::fs::page_cache_registry_count(),
        crate::fs::dirty_owner_count(),
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
        "context_switches={} local_sfences={} remote_rfences={} full_tlb_invalidations={} asid_tlb_invalidations={} scheduler_ipis={} ipis_received={} scheduler_yields={} syscall_yields={} quiescence_yields={} timer_preemptions={} blocking_switches={} task_running_ticks={} idle_ticks={}",
        s.context_switches,
        s.local_sfences,
        s.remote_rfences,
        s.full_tlb_invalidations,
        s.asid_tlb_invalidations,
        s.scheduler_ipis,
        s.ipis_received,
        s.scheduler_yields,
        s.syscall_yields,
        s.quiescence_yields,
        s.timer_preemptions,
        s.blocking_switches,
        s.task_running_ticks,
        s.idle_ticks
    );
    let _ = writeln!(
        out,
        "local_sfence_ticks={} local_sfence_max_ticks={} tlb_flush_calls={} tlb_fresh_map_flushes={} tlb_cow_flushes={} tlb_flush_retired_batches={} tlb_flush_retired_frames={}",
        s.local_sfence_ticks,
        s.local_sfence_max_ticks,
        s.tlb_flush_calls,
        s.tlb_fresh_map_flushes,
        s.tlb_cow_flushes,
        s.tlb_flush_retired_batches,
        s.tlb_flush_retired_frames
    );
    let _ = writeln!(
        out,
        "tlb_shootdown_all_requests={} tlb_shootdown_address_space_requests={} tlb_shootdown_range_requests={} tlb_shootdown_range_pages={} tlb_shootdown_range_max_pages={} tlb_shootdown_range_single_page={} tlb_shootdown_range_le16_pages={} tlb_shootdown_range_le256_pages={} tlb_shootdown_range_gt256_pages={} tlb_shootdown_invalid_requests={}",
        s.tlb_shootdown_all_requests,
        s.tlb_shootdown_address_space_requests,
        s.tlb_shootdown_range_requests,
        s.tlb_shootdown_range_pages,
        s.tlb_shootdown_range_max_pages,
        s.tlb_shootdown_range_single_page,
        s.tlb_shootdown_range_le16_pages,
        s.tlb_shootdown_range_le256_pages,
        s.tlb_shootdown_range_gt256_pages,
        s.tlb_shootdown_invalid_requests
    );
    let _ = writeln!(
        out,
        "remote_rfence_target_harts={} remote_rfence_empty_requests={} remote_rfence_wait_ticks={} remote_rfence_max_wait_ticks={}",
        s.remote_rfence_target_harts,
        s.remote_rfence_empty_requests,
        s.remote_rfence_wait_ticks,
        s.remote_rfence_max_wait_ticks
    );
    let _ = writeln!(
        out,
        "scheduler_lock_acquisitions={} scheduler_lock_wait_ticks={} scheduler_lock_max_wait_ticks={} scheduler_ready_peak={}",
        s.scheduler_lock_acquisitions,
        s.scheduler_lock_wait_ticks,
        s.scheduler_lock_max_wait_ticks,
        s.scheduler_ready_peak
    );
    let _ = writeln!(
        out,
        "concurrency_samples={} running_harts_0={} running_harts_1={} running_harts_2_3={} running_harts_4_7={} running_harts_8_plus={} scheduler_ready_0={} scheduler_ready_1={} scheduler_ready_2_3={} scheduler_ready_4_7={} scheduler_ready_8_plus={}",
        s.concurrency_samples,
        s.running_harts_0,
        s.running_harts_1,
        s.running_harts_2_3,
        s.running_harts_4_7,
        s.running_harts_8_plus,
        s.scheduler_ready_0,
        s.scheduler_ready_1,
        s.scheduler_ready_2_3,
        s.scheduler_ready_4_7,
        s.scheduler_ready_8_plus
    );
    let _ = writeln!(
        out,
        "user_traps={} syscall={} page_fault={} timer={} ipi={} extension_state_eager_saves={}",
        s.user_traps,
        s.user_syscall_traps,
        s.user_page_fault_traps,
        s.user_timer_traps,
        s.user_ipi_traps,
        s.extension_state_eager_saves
    );
    let _ = writeln!(
        out,
        "yield_breakdown_fs={} stdio={} tty={} pipe={} fs_syscall={} futex={} net={} process={} signal_time={} special_fd={}",
        s.fs_yields,
        s.stdio_yields,
        s.tty_yields,
        s.pipe_yields,
        s.fs_syscall_yields,
        s.futex_yields,
        s.net_yields,
        s.process_yields,
        s.signal_time_yields,
        s.special_fd_yields
    );
    let _ = writeln!(
        out,
        "net_yield_breakdown_unix={} tcp_wait={} tcp_connect={} udp_wait={}",
        s.unix_socket_yields, s.tcp_wait_yields, s.tcp_connect_yields, s.udp_wait_yields
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
        "ext4_lock_acquisitions_by_class_stat={} lookup={} read={} write={} readdir={} namespace={} attributes={} superblock={}",
        s.ext4_lock_stat_acquisitions,
        s.ext4_lock_lookup_acquisitions,
        s.ext4_lock_read_acquisitions,
        s.ext4_lock_write_acquisitions,
        s.ext4_lock_readdir_acquisitions,
        s.ext4_lock_namespace_acquisitions,
        s.ext4_lock_attributes_acquisitions,
        s.ext4_lock_superblock_acquisitions
    );
    let _ = writeln!(
        out,
        "ext4_lock_wait_by_class_stat={} lookup={} read={} write={} readdir={} namespace={} attributes={} superblock={}",
        s.ext4_lock_stat_wait_ticks,
        s.ext4_lock_lookup_wait_ticks,
        s.ext4_lock_read_wait_ticks,
        s.ext4_lock_write_wait_ticks,
        s.ext4_lock_readdir_wait_ticks,
        s.ext4_lock_namespace_wait_ticks,
        s.ext4_lock_attributes_wait_ticks,
        s.ext4_lock_superblock_wait_ticks
    );
    let _ = writeln!(
        out,
        "ext4_lock_hold_by_class_stat={} lookup={} read={} write={} readdir={} namespace={} attributes={} superblock={}",
        s.ext4_lock_stat_hold_ticks,
        s.ext4_lock_lookup_hold_ticks,
        s.ext4_lock_read_hold_ticks,
        s.ext4_lock_write_hold_ticks,
        s.ext4_lock_readdir_hold_ticks,
        s.ext4_lock_namespace_hold_ticks,
        s.ext4_lock_attributes_hold_ticks,
        s.ext4_lock_superblock_hold_ticks
    );
    let _ = writeln!(
        out,
        "ext4_profiled_lower_calls={} ticks={} by_class_stat={} lookup={} read={} write={} readdir={} namespace={} attributes={} superblock={}",
        s.ext4_lower_calls,
        s.ext4_lower_ticks,
        s.ext4_lower_stat_calls,
        s.ext4_lower_lookup_calls,
        s.ext4_lower_read_calls,
        s.ext4_lower_write_calls,
        s.ext4_lower_readdir_calls,
        s.ext4_lower_namespace_calls,
        s.ext4_lower_attributes_calls,
        s.ext4_lower_superblock_calls
    );
    let _ = writeln!(
        out,
        "ext4_profiled_lower_ticks_by_class_stat={} lookup={} read={} write={} readdir={} namespace={} attributes={} superblock={}",
        s.ext4_lower_stat_ticks,
        s.ext4_lower_lookup_ticks,
        s.ext4_lower_read_ticks,
        s.ext4_lower_write_ticks,
        s.ext4_lower_readdir_ticks,
        s.ext4_lower_namespace_ticks,
        s.ext4_lower_attributes_ticks,
        s.ext4_lower_superblock_ticks
    );
    let _ = writeln!(
        out,
        "ext4_ops_stat_calls={} stat_ticks={} stat_cache_hits={} stat_cache_misses={} stat_cache_refills={} stat_cache_uncacheable={} stat_cache_invalidations={} lookup_calls={} lookup_ticks={} readdir_calls={} readdir_ticks={} readdir_dirent_type_known={} readdir_dirent_type_unknown={} create_calls={} create_ticks={} write_calls={} write_ticks={}",
        s.ext4_stat_calls,
        s.ext4_stat_ticks,
        s.ext4_stat_cache_hits,
        s.ext4_stat_cache_misses,
        s.ext4_stat_cache_refills,
        s.ext4_stat_cache_uncacheable,
        s.ext4_stat_cache_invalidations,
        s.ext4_lookup_calls,
        s.ext4_lookup_ticks,
        s.ext4_readdir_calls,
        s.ext4_readdir_ticks,
        s.ext4_readdir_dirent_type_known,
        s.ext4_readdir_dirent_type_unknown,
        s.ext4_create_calls,
        s.ext4_create_ticks,
        s.ext4_write_calls,
        s.ext4_write_ticks
    );
    let _ = writeln!(
        out,
        "ext4_attributes_set_times_calls={} atime_updates={} mtime_updates={} set_mode_calls={} set_owner_calls={}",
        s.ext4_set_times_calls,
        s.ext4_set_times_atime_updates,
        s.ext4_set_times_mtime_updates,
        s.ext4_set_mode_calls,
        s.ext4_set_owner_calls
    );
    let _ = writeln!(
        out,
        "heap_alloc_calls={} heap_dealloc_calls={} heap_alloc_bytes={} heap_dealloc_bytes={} heap_current_bytes={} heap_peak_bytes={}",
        class_alloc_calls,
        class_dealloc_calls,
        class_alloc_bytes,
        class_dealloc_bytes,
        heap_current_bytes,
        heap_peak_bytes
    );
    let _ = writeln!(
        out,
        "heap_peak_exact={} heap_magazine_enabled={} heap_magazine_cached_bytes={} heap_magazine_cached_peak_upper_bound_bytes={}",
        usize::from(heap_peak_exact),
        usize::from(cfg!(feature = "heap_magazine")),
        magazine_cached_bytes,
        magazine_cached_peak_bytes
    );
    let _ = writeln!(
        out,
        "dentry_cache_hits={} dentry_cache_misses={} dentry_cache_evictions={}",
        s.dentry_cache_hits, s.dentry_cache_misses, s.dentry_cache_evictions
    );
    let _ = writeln!(
        out,
        "heap_alloc_ticks={} heap_dealloc_ticks={} heap_max_alloc_ticks={} heap_max_dealloc_ticks={} heap_high_alignment_allocs={} clock_hz={}",
        heap_classes.alloc_ticks,
        heap_classes.dealloc_ticks,
        heap_classes.max_alloc_ticks,
        heap_classes.max_dealloc_ticks,
        heap_classes.high_alignment_allocs,
        crate::timer::get_hardware_clock_freq()
    );
    let _ = writeln!(
        out,
        "heap_timing_sample_rate={} heap_alloc_timing_samples={} heap_dealloc_timing_samples={} heap_timing_estimated=1",
        HEAP_TIMING_SAMPLE_RATE,
        heap_classes.alloc_timing_samples,
        heap_classes.dealloc_timing_samples
    );
    let _ = writeln!(
        out,
        "heap_magazine_hits={} misses={} cached_frees={} refill_blocks={} overflow_returns={} reclaim_blocks={}",
        heap_classes.magazine_hits,
        heap_classes.magazine_misses,
        heap_classes.magazine_cached_frees,
        heap_classes.magazine_refill_blocks,
        heap_classes.magazine_overflow_returns,
        heap_classes.magazine_reclaim_blocks
    );
    let _ = writeln!(
        out,
        "heap_alloc_lock_wait_ticks={} heap_dealloc_lock_wait_ticks={} heap_alloc_core_ticks={} heap_dealloc_core_ticks={}",
        class_alloc_wait_ticks,
        class_dealloc_wait_ticks,
        class_alloc_core_ticks,
        class_dealloc_core_ticks
    );
    for (idx, upper) in HEAP_CLASS_UPPER_BOUNDS.iter().enumerate() {
        // `upper=0` denotes the final unbounded (>4096) class without relying
        // on target-width-dependent usize::MAX text in parsers.
        let upper_label = if *upper == usize::MAX { 0 } else { *upper };
        let _ = writeln!(
            out,
            "heap_class={} upper={} alloc_calls={} dealloc_calls={} alloc_bytes={} dealloc_bytes={} alloc_wait_ticks={} dealloc_wait_ticks={} alloc_core_ticks={} dealloc_core_ticks={}",
            idx,
            upper_label,
            heap_classes.alloc_calls[idx],
            heap_classes.dealloc_calls[idx],
            heap_classes.alloc_bytes[idx],
            heap_classes.dealloc_bytes[idx],
            heap_classes.alloc_wait_ticks[idx],
            heap_classes.dealloc_wait_ticks[idx],
            heap_classes.alloc_core_ticks[idx],
            heap_classes.dealloc_core_ticks[idx]
        );
    }
    let _ = writeln!(
        out,
        "heap_class_totals alloc_calls={} dealloc_calls={} alloc_bytes={} dealloc_bytes={} alloc_wait_ticks={} dealloc_wait_ticks={} alloc_core_ticks={} dealloc_core_ticks={} match_totals={}",
        class_alloc_calls,
        class_dealloc_calls,
        class_alloc_bytes,
        class_dealloc_bytes,
        class_alloc_wait_ticks,
        class_dealloc_wait_ticks,
        class_alloc_core_ticks,
        class_dealloc_core_ticks,
        1
    );
    let _ = writeln!(
        out,
        "frame_alloc_calls={} failures={} alloc_ticks={} lock_wait_ticks={} core_ticks={} clear_ticks={} max_ticks={}",
        s.frame_alloc_calls,
        s.frame_alloc_failures,
        s.frame_alloc_ticks,
        s.frame_alloc_lock_wait_ticks,
        s.frame_alloc_core_ticks,
        s.frame_alloc_clear_ticks,
        s.frame_alloc_max_ticks
    );
    let _ = writeln!(
        out,
        "frame_dealloc_calls={} dealloc_ticks={} lock_wait_ticks={} core_ticks={} max_ticks={}",
        s.frame_dealloc_calls,
        s.frame_dealloc_ticks,
        s.frame_dealloc_lock_wait_ticks,
        s.frame_dealloc_core_ticks,
        s.frame_dealloc_max_ticks
    );
    let _ = writeln!(
        out,
        "copy_from_user_calls={} copy_from_user_bytes={} copy_from_user_ticks={} copy_to_user_calls={} copy_to_user_bytes={} copy_to_user_ticks={}",
        s.copy_from_user_calls,
        s.copy_from_user_bytes,
        s.copy_from_user_ticks,
        s.copy_to_user_calls,
        s.copy_to_user_bytes,
        s.copy_to_user_ticks
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

/// Select one out of every `HEAP_TIMING_SAMPLE_RATE` heap operations per hart.
/// Exact calls/bytes remain unsampled; only the expensive clock-based timing is
/// estimated from these samples.
#[inline(always)]
pub fn heap_timing_start() -> usize {
    #[cfg(feature = "perf_counters")]
    {
        let sequence = current_heap_classes()
            .timing_sequence
            .fetch_add(1, Ordering::Relaxed);
        if sequence % HEAP_TIMING_SAMPLE_RATE == 0 {
            now_ticks()
        } else {
            0
        }
    }
    #[cfg(not(feature = "perf_counters"))]
    {
        0
    }
}

#[inline(always)]
pub fn heap_timing_checkpoint(start: usize) -> usize {
    if start == 0 { 0 } else { now_ticks() }
}

macro_rules! heap_magazine_increment_functions {
    ($(($fn_name:ident, $field:ident)),+ $(,)?) => {
        $(
            #[inline(always)]
            pub fn $fn_name(value: usize) {
                #[cfg(feature = "perf_counters")]
                current_heap_classes().$field.fetch_add(value, Ordering::Relaxed);
                #[cfg(not(feature = "perf_counters"))]
                let _ = value;
            }
        )+
    };
}

heap_magazine_increment_functions!(
    (heap_magazine_hit, magazine_hits),
    (heap_magazine_miss, magazine_misses),
    (heap_magazine_cached_free, magazine_cached_frees),
    (heap_magazine_refill_blocks, magazine_refill_blocks),
    (heap_magazine_overflow_return, magazine_overflow_returns),
    (heap_magazine_reclaim_blocks, magazine_reclaim_blocks),
);

#[inline(always)]
pub fn elapsed_since(start: usize) -> usize {
    now_ticks().wrapping_sub(start)
}
