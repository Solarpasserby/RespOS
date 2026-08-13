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
    remote_rfences,
    full_tlb_invalidations,
    asid_tlb_invalidations,
    tlb_shootdown_all_requests,
    tlb_shootdown_address_space_requests,
    tlb_shootdown_range_requests,
    tlb_shootdown_invalid_requests,
    scheduler_ipis,
    ipis_received,
    scheduler_lock_acquisitions,
    scheduler_lock_wait_ticks,
    scheduler_lock_max_wait_ticks,
    scheduler_ready_peak,
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
    heap_alloc_calls,
    heap_dealloc_calls,
    heap_alloc_bytes,
    heap_dealloc_bytes,
    heap_current_bytes,
    heap_peak_bytes,
    heap_alloc_ticks,
    heap_dealloc_ticks,
    heap_alloc_lock_wait_ticks,
    heap_dealloc_lock_wait_ticks,
    heap_alloc_core_ticks,
    heap_dealloc_core_ticks,
    heap_max_alloc_ticks,
    heap_max_dealloc_ticks,
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
    (remote_rfence, remote_rfences),
    (full_tlb_invalidation, full_tlb_invalidations),
    (asid_tlb_invalidation, asid_tlb_invalidations),
    (tlb_shootdown_all_request, tlb_shootdown_all_requests),
    (
        tlb_shootdown_address_space_request,
        tlb_shootdown_address_space_requests
    ),
    (tlb_shootdown_range_request, tlb_shootdown_range_requests),
    (
        tlb_shootdown_invalid_request,
        tlb_shootdown_invalid_requests
    ),
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
);

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
pub fn observe_scheduler_ready(value: usize) {
    #[cfg(feature = "perf_counters")]
    observe_max(&COUNTERS.scheduler_ready_peak, value);
    #[cfg(not(feature = "perf_counters"))]
    let _ = value;
}

#[inline(always)]
pub fn heap_alloc(
    size: usize,
    ticks: usize,
    lock_wait_ticks: usize,
    core_ticks: usize,
    succeeded: bool,
) {
    #[cfg(feature = "perf_counters")]
    {
        COUNTERS.heap_alloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .heap_alloc_ticks
            .fetch_add(ticks, Ordering::Relaxed);
        COUNTERS
            .heap_alloc_lock_wait_ticks
            .fetch_add(lock_wait_ticks, Ordering::Relaxed);
        COUNTERS
            .heap_alloc_core_ticks
            .fetch_add(core_ticks, Ordering::Relaxed);
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
    let _ = (size, ticks, lock_wait_ticks, core_ticks, succeeded);
}

#[inline(always)]
pub fn heap_dealloc(size: usize, ticks: usize, lock_wait_ticks: usize, core_ticks: usize) {
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
            .heap_dealloc_lock_wait_ticks
            .fetch_add(lock_wait_ticks, Ordering::Relaxed);
        COUNTERS
            .heap_dealloc_core_ticks
            .fetch_add(core_ticks, Ordering::Relaxed);
        COUNTERS
            .heap_current_bytes
            .fetch_sub(size, Ordering::Relaxed);
        observe_max(&COUNTERS.heap_max_dealloc_ticks, ticks);
    }
    #[cfg(not(feature = "perf_counters"))]
    let _ = (size, ticks, lock_wait_ticks, core_ticks);
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
        "tlb_shootdown_all_requests={} tlb_shootdown_address_space_requests={} tlb_shootdown_range_requests={} tlb_shootdown_invalid_requests={}",
        s.tlb_shootdown_all_requests,
        s.tlb_shootdown_address_space_requests,
        s.tlb_shootdown_range_requests,
        s.tlb_shootdown_invalid_requests
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
        "ext4_ops_stat_calls={} stat_ticks={} stat_cache_hits={} stat_cache_misses={} stat_cache_refills={} stat_cache_uncacheable={} stat_cache_invalidations={} lookup_calls={} lookup_ticks={} readdir_calls={} readdir_ticks={} create_calls={} create_ticks={} write_calls={} write_ticks={}",
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
        s.heap_alloc_calls,
        s.heap_dealloc_calls,
        s.heap_alloc_bytes,
        s.heap_dealloc_bytes,
        s.heap_current_bytes,
        s.heap_peak_bytes
    );
    let _ = writeln!(
        out,
        "dentry_cache_hits={} dentry_cache_misses={} dentry_cache_evictions={}",
        s.dentry_cache_hits, s.dentry_cache_misses, s.dentry_cache_evictions
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
    let _ = writeln!(
        out,
        "heap_alloc_lock_wait_ticks={} heap_dealloc_lock_wait_ticks={} heap_alloc_core_ticks={} heap_dealloc_core_ticks={}",
        s.heap_alloc_lock_wait_ticks,
        s.heap_dealloc_lock_wait_ticks,
        s.heap_alloc_core_ticks,
        s.heap_dealloc_core_ticks
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

#[inline(always)]
pub fn elapsed_since(start: usize) -> usize {
    now_ticks().wrapping_sub(start)
}
