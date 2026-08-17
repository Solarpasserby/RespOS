use super::TaskControlBlock;
use crate::mutex::SpinNoIrqLock;
use crate::signal::sig_struct::SigPending;
use crate::signal::SigInfo;
use alloc::collections::btree_map::BTreeMap;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize, Ordering};
use hashbrown::HashMap;
use lazy_static::lazy_static;

lazy_static! {
    /// Stable process identities indexed by PID/TGID.  The table deliberately
    /// stores weak references: live members and the parent children table are
    /// the lifetime owners, including the Zombie-to-Reaped wait interval.
    pub static ref PROCESS_MANAGER: ProcessManager = ProcessManager::new();
}

/// Non-time fields reported by getrusage(2).  A snapshot is also retained by
/// a zombie until wait4 successfully reaps it.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResourceUsageSnapshot {
    pub maxrss_pages: usize,
    pub minflt: usize,
    pub majflt: usize,
    pub inblock: usize,
    pub oublock: usize,
    pub nvcsw: usize,
    pub nivcsw: usize,
}

pub struct ResourceUsageCounters {
    maxrss_pages: AtomicUsize,
    minflt: AtomicUsize,
    majflt: AtomicUsize,
    inblock: AtomicUsize,
    oublock: AtomicUsize,
    nvcsw: AtomicUsize,
    nivcsw: AtomicUsize,
}

impl ResourceUsageCounters {
    pub fn new() -> Self {
        Self {
            maxrss_pages: AtomicUsize::new(0),
            minflt: AtomicUsize::new(0),
            majflt: AtomicUsize::new(0),
            inblock: AtomicUsize::new(0),
            oublock: AtomicUsize::new(0),
            nvcsw: AtomicUsize::new(0),
            nivcsw: AtomicUsize::new(0),
        }
    }

    pub fn snapshot(&self) -> ResourceUsageSnapshot {
        ResourceUsageSnapshot {
            maxrss_pages: self.maxrss_pages.load(Ordering::Acquire),
            minflt: self.minflt.load(Ordering::Acquire),
            majflt: self.majflt.load(Ordering::Acquire),
            inblock: self.inblock.load(Ordering::Acquire),
            oublock: self.oublock.load(Ordering::Acquire),
            nvcsw: self.nvcsw.load(Ordering::Acquire),
            nivcsw: self.nivcsw.load(Ordering::Acquire),
        }
    }

    pub fn note_maxrss_pages(&self, pages: usize) {
        self.maxrss_pages.fetch_max(pages, Ordering::AcqRel);
    }

    pub fn note_minor_fault(&self) {
        self.minflt.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_major_fault(&self) {
        self.majflt.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_input_blocks(&self, blocks: usize) {
        self.inblock.fetch_add(blocks, Ordering::Relaxed);
    }

    pub fn note_output_blocks(&self, blocks: usize) {
        self.oublock.fetch_add(blocks, Ordering::Relaxed);
    }

    pub fn note_voluntary_context_switch(&self) {
        self.nvcsw.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_involuntary_context_switch(&self) {
        self.nivcsw.fetch_add(1, Ordering::Relaxed);
    }

    /// Reaped-child accounting sums counters but retains the largest child
    /// high-water RSS, matching Linux RUSAGE_CHILDREN.
    fn add_child(&self, usage: ResourceUsageSnapshot) {
        self.note_maxrss_pages(usage.maxrss_pages);
        self.minflt.fetch_add(usage.minflt, Ordering::Relaxed);
        self.majflt.fetch_add(usage.majflt, Ordering::Relaxed);
        self.inblock.fetch_add(usage.inblock, Ordering::Relaxed);
        self.oublock.fetch_add(usage.oublock, Ordering::Relaxed);
        self.nvcsw.fetch_add(usage.nvcsw, Ordering::Relaxed);
        self.nivcsw.fetch_add(usage.nivcsw, Ordering::Relaxed);
    }
}

/// Process-wide identity which remains valid independently of which member
/// thread currently owns the numeric TGID.
pub struct ProcessState {
    tgid: usize,
    pgid: AtomicUsize,
    sid: AtomicUsize,
    uid: AtomicUsize,
    euid: AtomicUsize,
    suid: AtomicUsize,
    controlling_tty: AtomicBool,
    members: SpinNoIrqLock<BTreeMap<usize, Weak<TaskControlBlock>>>,
    parent: SpinNoIrqLock<Option<Weak<ProcessState>>>,
    children: SpinNoIrqLock<BTreeMap<usize, Arc<ProcessState>>>,
    exited_children: SpinNoIrqLock<alloc::collections::btree_set::BTreeSet<usize>>,
    lifecycle: AtomicU8,
    wait_status: AtomicI32,
    wait_event_code: AtomicI32,
    wait_event_status: AtomicI32,
    user_ticks: AtomicUsize,
    system_ticks: AtomicUsize,
    child_utime_ticks: AtomicUsize,
    child_stime_ticks: AtomicUsize,
    resource_usage: ResourceUsageCounters,
    child_resource_usage: ResourceUsageCounters,
    child_event_generation: AtomicUsize,
    did_exec: AtomicBool,
    process_pending: SpinNoIrqLock<SigPending>,
    queued_rt_signals: AtomicUsize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ProcessLifecycle {
    Running = 0,
    Exec = 1,
    Exiting = 2,
    Zombie = 3,
    Reaped = 4,
}

impl ProcessState {
    pub fn new(
        tgid: usize,
        pgid: usize,
        sid: usize,
        uid: usize,
        euid: usize,
        suid: usize,
        controlling_tty: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            tgid,
            pgid: AtomicUsize::new(pgid),
            sid: AtomicUsize::new(sid),
            uid: AtomicUsize::new(uid),
            euid: AtomicUsize::new(euid),
            suid: AtomicUsize::new(suid),
            controlling_tty: AtomicBool::new(controlling_tty),
            members: SpinNoIrqLock::new(BTreeMap::new()),
            parent: SpinNoIrqLock::new(None),
            children: SpinNoIrqLock::new(BTreeMap::new()),
            exited_children: SpinNoIrqLock::new(Default::default()),
            lifecycle: AtomicU8::new(ProcessLifecycle::Running as u8),
            wait_status: AtomicI32::new(0),
            wait_event_code: AtomicI32::new(0),
            wait_event_status: AtomicI32::new(0),
            user_ticks: AtomicUsize::new(0),
            system_ticks: AtomicUsize::new(0),
            child_utime_ticks: AtomicUsize::new(0),
            child_stime_ticks: AtomicUsize::new(0),
            resource_usage: ResourceUsageCounters::new(),
            child_resource_usage: ResourceUsageCounters::new(),
            child_event_generation: AtomicUsize::new(0),
            did_exec: AtomicBool::new(false),
            process_pending: SpinNoIrqLock::new(SigPending::new()),
            queued_rt_signals: AtomicUsize::new(0),
        })
    }

    pub fn tgid(&self) -> usize {
        self.tgid
    }

    pub fn pgid(&self) -> usize {
        self.pgid.load(Ordering::Acquire)
    }

    pub fn sid(&self) -> usize {
        self.sid.load(Ordering::Acquire)
    }

    pub fn set_pgid(&self, pgid: usize) {
        self.pgid.store(pgid, Ordering::Release);
    }

    pub fn set_sid(&self, sid: usize) {
        self.sid.store(sid, Ordering::Release);
    }

    pub fn uid(&self) -> usize {
        self.uid.load(Ordering::Acquire)
    }

    pub fn euid(&self) -> usize {
        self.euid.load(Ordering::Acquire)
    }

    pub fn suid(&self) -> usize {
        self.suid.load(Ordering::Acquire)
    }

    pub fn set_uid_triplet(&self, uid: usize, euid: usize, suid: usize) {
        self.uid.store(uid, Ordering::Release);
        self.euid.store(euid, Ordering::Release);
        self.suid.store(suid, Ordering::Release);
    }

    pub fn has_controlling_tty(&self) -> bool {
        self.controlling_tty.load(Ordering::Acquire)
    }

    pub fn set_controlling_tty(&self, present: bool) {
        self.controlling_tty.store(present, Ordering::Release);
    }

    pub fn add_member(&self, task: &Arc<TaskControlBlock>) {
        debug_assert_eq!(task.tgid(), self.tgid);
        self.members.lock().insert(task.tid(), Arc::downgrade(task));
    }

    pub fn remove_member(&self, tid: usize) {
        self.members.lock().remove(&tid);
    }

    pub fn member(&self, tid: usize) -> Option<Arc<TaskControlBlock>> {
        self.members.lock().get(&tid).and_then(Weak::upgrade)
    }

    pub fn members(&self) -> alloc::vec::Vec<Arc<TaskControlBlock>> {
        self.members
            .lock()
            .values()
            .filter_map(Weak::upgrade)
            .collect()
    }

    /// Select the traditional leader while it exists, otherwise a surviving
    /// member. Process-pending storage is independent of this wake/delivery
    /// hint and therefore does not depend on a leader TCB remaining alive.
    pub fn signal_target(&self) -> Option<Arc<TaskControlBlock>> {
        self.member(self.tgid)
            .or_else(|| self.members().into_iter().next())
    }

    pub fn add_pending_signal(&self, siginfo: SigInfo) -> bool {
        self.process_pending.lock().add_signal(siginfo)
    }

    /// Reserve one queued real-time signal against this process's shared
    /// RLIMIT_SIGPENDING view. Linux accounts by real UID across processes;
    /// RespOS currently has no global UID object, so the stable ProcessState is
    /// the narrowest race-safe owner until that credential layer exists.
    pub fn reserve_rt_signal(&self, limit: usize) -> bool {
        let mut count = self.queued_rt_signals.load(Ordering::Acquire);
        loop {
            if count >= limit {
                return false;
            }
            match self.queued_rt_signals.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => count = current,
            }
        }
    }

    pub fn release_rt_signals(&self, count: usize) {
        if count == 0 {
            return;
        }
        let previous = self.queued_rt_signals.fetch_sub(count, Ordering::AcqRel);
        debug_assert!(previous >= count);
    }

    pub fn op_sig_pending<T>(&self, f: impl FnOnce(&SigPending) -> T) -> T {
        f(&self.process_pending.lock())
    }

    pub fn op_sig_pending_mut<T>(&self, f: impl FnOnce(&mut SigPending) -> T) -> T {
        f(&mut self.process_pending.lock())
    }

    pub fn set_parent(&self, parent: &Arc<ProcessState>) {
        *self.parent.lock() = Some(Arc::downgrade(parent));
    }

    pub fn parent(&self) -> Option<Arc<ProcessState>> {
        self.parent.lock().as_ref().and_then(Weak::upgrade)
    }

    pub fn add_child(self: &Arc<Self>, child: Arc<ProcessState>) {
        child.set_parent(self);
        self.children.lock().insert(child.tgid(), child);
    }

    pub fn op_children_mut<T>(
        &self,
        f: impl FnOnce(&mut BTreeMap<usize, Arc<ProcessState>>) -> T,
    ) -> T {
        f(&mut self.children.lock())
    }

    pub fn exited_child_ids(&self) -> alloc::vec::Vec<usize> {
        self.exited_children.lock().iter().copied().collect()
    }

    pub fn mark_child_exited(&self, tgid: usize) {
        self.exited_children.lock().insert(tgid);
    }

    pub fn remove_exited_child(&self, tgid: usize) {
        self.exited_children.lock().remove(&tgid);
    }

    pub fn clear_exited_children(&self) {
        self.exited_children.lock().clear();
    }

    pub fn lifecycle(&self) -> ProcessLifecycle {
        match self.lifecycle.load(Ordering::Acquire) {
            0 => ProcessLifecycle::Running,
            1 => ProcessLifecycle::Exec,
            2 => ProcessLifecycle::Exiting,
            3 => ProcessLifecycle::Zombie,
            4 => ProcessLifecycle::Reaped,
            state => panic!("invalid process lifecycle {state}"),
        }
    }

    pub fn is_exited(&self) -> bool {
        self.lifecycle() == ProcessLifecycle::Zombie
    }

    pub fn begin_exec(&self) -> bool {
        self.lifecycle
            .compare_exchange(
                ProcessLifecycle::Running as u8,
                ProcessLifecycle::Exec as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub fn finish_exec(&self) {
        self.lifecycle
            .compare_exchange(
                ProcessLifecycle::Exec as u8,
                ProcessLifecycle::Running as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .expect("exec transition lost process ownership");
    }

    pub fn begin_exit(&self) -> bool {
        self.lifecycle
            .compare_exchange(
                ProcessLifecycle::Running as u8,
                ProcessLifecycle::Exiting as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub fn publish_zombie(&self, wait_status: i32, user_ticks: usize, system_ticks: usize) {
        self.wait_status.store(wait_status, Ordering::Relaxed);
        self.user_ticks.store(user_ticks, Ordering::Relaxed);
        self.system_ticks.store(system_ticks, Ordering::Relaxed);
        self.lifecycle
            .store(ProcessLifecycle::Zombie as u8, Ordering::Release);
    }

    pub fn mark_reaped(&self) {
        self.lifecycle
            .store(ProcessLifecycle::Reaped as u8, Ordering::Release);
    }

    pub fn wait_status(&self) -> i32 {
        self.wait_status.load(Ordering::Acquire)
    }

    pub fn set_wait_status(&self, status: i32) {
        self.wait_status.store(status, Ordering::Release);
    }

    pub fn accounting_ticks(&self) -> (usize, usize) {
        (
            self.user_ticks.load(Ordering::Acquire),
            self.system_ticks.load(Ordering::Acquire),
        )
    }

    pub fn set_wait_event(&self, code: i32, status: i32) {
        self.wait_event_status.store(status, Ordering::Relaxed);
        self.wait_event_code.store(code, Ordering::Release);
    }

    pub fn take_wait_event(&self) -> Option<(i32, i32)> {
        let code = self.wait_event_code.swap(0, Ordering::AcqRel);
        (code != 0).then(|| (code, self.wait_event_status.load(Ordering::Acquire)))
    }

    pub fn peek_wait_event(&self) -> Option<(i32, i32)> {
        let code = self.wait_event_code.load(Ordering::Acquire);
        (code != 0).then(|| (code, self.wait_event_status.load(Ordering::Acquire)))
    }

    pub fn child_ticks(&self) -> (usize, usize) {
        (
            self.child_utime_ticks.load(Ordering::Relaxed),
            self.child_stime_ticks.load(Ordering::Relaxed),
        )
    }

    pub fn child_event_generation(&self) -> usize {
        self.child_event_generation.load(Ordering::Acquire)
    }

    pub fn publish_child_event(&self) {
        self.child_event_generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn add_child_ticks(&self, utime: usize, stime: usize) {
        self.child_utime_ticks.fetch_add(utime, Ordering::Relaxed);
        self.child_stime_ticks.fetch_add(stime, Ordering::Relaxed);
    }

    pub fn resource_usage(&self) -> ResourceUsageSnapshot {
        self.resource_usage.snapshot()
    }

    pub fn child_resource_usage(&self) -> ResourceUsageSnapshot {
        self.child_resource_usage.snapshot()
    }

    pub fn add_child_resource_usage(&self, usage: ResourceUsageSnapshot) {
        self.child_resource_usage.add_child(usage);
    }

    pub fn note_maxrss_pages(&self, pages: usize) {
        self.resource_usage.note_maxrss_pages(pages);
    }

    pub fn note_minor_fault(&self) {
        self.resource_usage.note_minor_fault();
    }

    pub fn note_major_fault(&self) {
        self.resource_usage.note_major_fault();
    }

    pub fn note_input_blocks(&self, blocks: usize) {
        self.resource_usage.note_input_blocks(blocks);
    }

    pub fn note_output_blocks(&self, blocks: usize) {
        self.resource_usage.note_output_blocks(blocks);
    }

    pub fn note_voluntary_context_switch(&self) {
        self.resource_usage.note_voluntary_context_switch();
    }

    pub fn note_involuntary_context_switch(&self) {
        self.resource_usage.note_involuntary_context_switch();
    }

    pub fn did_exec(&self) -> bool {
        self.did_exec.load(Ordering::Acquire)
    }

    pub fn mark_exec(&self) {
        self.did_exec.store(true, Ordering::Release);
    }
}

pub struct ProcessManager(SpinNoIrqLock<HashMap<usize, Weak<ProcessState>>>);

impl ProcessManager {
    pub fn new() -> Self {
        Self(SpinNoIrqLock::new(HashMap::new()))
    }

    pub fn add(&self, process: &Arc<ProcessState>) {
        self.0
            .lock()
            .insert(process.tgid(), Arc::downgrade(process));
    }

    pub fn remove(&self, tgid: usize) {
        self.0.lock().remove(&tgid);
    }

    pub fn get(&self, tgid: usize) -> Option<Arc<ProcessState>> {
        let mut processes = self.0.lock();
        let process = processes.get(&tgid).and_then(Weak::upgrade);
        if process.is_none() {
            processes.remove(&tgid);
        }
        process
    }

    pub fn for_each(&self, mut f: impl FnMut(&Arc<ProcessState>)) {
        let processes: alloc::vec::Vec<_> =
            self.0.lock().values().filter_map(Weak::upgrade).collect();
        for process in processes {
            f(&process);
        }
    }
}
