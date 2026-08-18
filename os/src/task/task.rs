// os/src/task/task.rs

//! 任务控制块及线程级生命周期的核心实现。
//!
//! `TaskControlBlock` 表示可被调度的线程；进程级身份、子进程和 zombie/reap 状态
//! 由 `process.rs` 管理。二者不能简单按 PID/TID 合并，否则 clone thread、group
//! exit、wait4 和 signal 的可观察语义会互相污染。
//!
//! 本文件负责或参与：任务创建与 fork/clone、exec 资源替换、内核栈和 trap
//! context、fd/MM/signal 共享关系、阻塞唤醒、退出清理及资源使用统计。修改时应明确：
//!
//! - 哪些字段属于线程，哪些属于 thread group，哪些由 `Arc` 在 fork 后共享；
//! - 状态转换由哪个锁线性化，谁是退出或唤醒的唯一胜者；
//! - 不能在持有 task/MM/FS 大锁时执行可能阻塞的后端 I/O；
//! - exec 成功前必须可回滚，成功提交后旧地址空间和 close-on-exec fd 才能释放；
//! - exit、signal kill 和 wait reap 是不同阶段，资源不能过早回收或重复清理。
#[cfg(target_arch = "loongarch64")]
use super::aux::AT_HWCAP;
use super::aux::{AuxHeader, AT_EXECFN, AT_NULL, AT_PLATFORM, AT_RANDOM};
use super::context::TaskContext;
use super::kstack::KernelStack;
use super::manager::TASK_MANAGER;
use super::process::{ProcessLifecycle, ProcessState, PROCESS_MANAGER};
use super::scheduler::remove_task;
use super::tid::{tid_alloc, TidHandle};
use super::INITPROC;
use crate::config::CLK_TCK;
use crate::fs::mount::init_root_fs;
use crate::fs::{FdEntry, FdTable, FileOp, Path};
use crate::mm::{copy_from_user, copy_to_user, writeback_file_pages, MemorySet};
use crate::mutex::{SpinLock, SpinNoIrqLock};
use crate::signal::sig_handler::{ActionType, SigActionFlag, SigHandler, SIG_IGN};
use crate::signal::sig_info::SigInfo;
use crate::signal::sig_stack::{SignalStack, SS_DISABLE};
use crate::signal::sig_struct::SigPending;
use crate::signal::{SiField, Sig, SigSet};
use crate::syscall::{Errno, SysResult};
use crate::timer::{get_accounting_clock_freq, get_time, get_timeout_ms};
use crate::trap::TrapContext;
use alloc::collections::btree_map::BTreeMap;
use alloc::collections::btree_set::BTreeSet;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use bitflags::bitflags;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use lazy_static::lazy_static;
use spin::{RwLock, RwLockReadGuard, RwLockWriteGuard};

lazy_static! {
    static ref ACTIVE_ITIMER_TASKS: SpinLock<BTreeSet<usize>> = SpinLock::new(BTreeSet::new());
}

const CPU_CLOCK_STOPPED: usize = usize::MAX;
const MAX_CPU_CLOCK_HARTS: usize = crate::arch::smp::MAX_HARTS;

#[derive(Default)]
struct ThreadCpuClock {
    accumulated_user_ticks: usize,
    accumulated_system_ticks: usize,
    running_since: Option<usize>,
    running_in_user: bool,
}

impl ThreadCpuClock {
    fn begin_run(&mut self, now: usize, in_user: bool) {
        debug_assert!(self.running_since.is_none());
        self.running_since = Some(now);
        self.running_in_user = in_user;
    }

    fn end_run(&mut self, now: usize) {
        let Some(started) = self.running_since.take() else {
            return;
        };
        let elapsed = now.wrapping_sub(started);
        if self.running_in_user {
            self.accumulated_user_ticks = self.accumulated_user_ticks.saturating_add(elapsed);
        } else {
            self.accumulated_system_ticks = self.accumulated_system_ticks.saturating_add(elapsed);
        }
    }

    fn transition(&mut self, now: usize, in_user: bool) {
        let Some(started) = self.running_since else {
            return;
        };
        if self.running_in_user == in_user {
            return;
        }
        let elapsed = now.wrapping_sub(started);
        if self.running_in_user {
            self.accumulated_user_ticks = self.accumulated_user_ticks.saturating_add(elapsed);
        } else {
            self.accumulated_system_ticks = self.accumulated_system_ticks.saturating_add(elapsed);
        }
        self.running_since = Some(now);
        self.running_in_user = in_user;
    }

    fn ticks_at(&self, now: usize) -> usize {
        let (user, system) = self.accounting_ticks_at(now);
        user.saturating_add(system)
    }

    fn accounting_ticks_at(&self, now: usize) -> (usize, usize) {
        let elapsed = self
            .running_since
            .map(|started| now.wrapping_sub(started))
            .unwrap_or(0);
        if self.running_in_user {
            (
                self.accumulated_user_ticks.saturating_add(elapsed),
                self.accumulated_system_ticks,
            )
        } else {
            (
                self.accumulated_user_ticks,
                self.accumulated_system_ticks.saturating_add(elapsed),
            )
        }
    }
}

struct ProcessCpuClock {
    accumulated_user_ticks: usize,
    accumulated_system_ticks: usize,
    running_since: [usize; MAX_CPU_CLOCK_HARTS],
    running_in_user: [bool; MAX_CPU_CLOCK_HARTS],
}

impl ProcessCpuClock {
    fn new() -> Self {
        Self {
            accumulated_user_ticks: 0,
            accumulated_system_ticks: 0,
            running_since: [CPU_CLOCK_STOPPED; MAX_CPU_CLOCK_HARTS],
            running_in_user: [true; MAX_CPU_CLOCK_HARTS],
        }
    }

    fn begin_run(&mut self, cpu: usize, now: usize, in_user: bool) {
        debug_assert_eq!(self.running_since[cpu], CPU_CLOCK_STOPPED);
        self.running_since[cpu] = now;
        self.running_in_user[cpu] = in_user;
    }

    fn end_run(&mut self, cpu: usize, now: usize) {
        let started = core::mem::replace(&mut self.running_since[cpu], CPU_CLOCK_STOPPED);
        if started == CPU_CLOCK_STOPPED {
            return;
        }
        let elapsed = now.wrapping_sub(started);
        if self.running_in_user[cpu] {
            self.accumulated_user_ticks = self.accumulated_user_ticks.saturating_add(elapsed);
        } else {
            self.accumulated_system_ticks = self.accumulated_system_ticks.saturating_add(elapsed);
        }
    }

    fn transition(&mut self, cpu: usize, now: usize, in_user: bool) {
        let started = self.running_since[cpu];
        if started == CPU_CLOCK_STOPPED || self.running_in_user[cpu] == in_user {
            return;
        }
        let elapsed = now.wrapping_sub(started);
        if self.running_in_user[cpu] {
            self.accumulated_user_ticks = self.accumulated_user_ticks.saturating_add(elapsed);
        } else {
            self.accumulated_system_ticks = self.accumulated_system_ticks.saturating_add(elapsed);
        }
        self.running_since[cpu] = now;
        self.running_in_user[cpu] = in_user;
    }

    fn ticks_at(&self, now: usize) -> usize {
        let (user, system) = self.accounting_ticks_at(now);
        user.saturating_add(system)
    }

    fn accounting_ticks_at(&self, now: usize) -> (usize, usize) {
        self.running_since
            .iter()
            .enumerate()
            .filter(|(_, started)| **started != CPU_CLOCK_STOPPED)
            .fold(
                (self.accumulated_user_ticks, self.accumulated_system_ticks),
                |(user, system), (cpu, started)| {
                    let elapsed = now.wrapping_sub(*started);
                    if self.running_in_user[cpu] {
                        (user.saturating_add(elapsed), system)
                    } else {
                        (user, system.saturating_add(elapsed))
                    }
                },
            )
    }
}

#[derive(Clone)]
enum CpuClockSource {
    Thread(Arc<SpinNoIrqLock<ThreadCpuClock>>),
    Process(Arc<SpinNoIrqLock<ProcessCpuClock>>),
}

/// POSIX timer 保留的分离 clock 引用；线程退出后无需继续持有完整 task 或其地址空间。
#[derive(Clone)]
pub struct CpuClockHandle {
    source: CpuClockSource,
}

impl CpuClockHandle {
    pub fn now_us(&self) -> usize {
        let now = get_time();
        let ticks = match &self.source {
            CpuClockSource::Thread(clock) => clock.lock().ticks_at(now),
            CpuClockSource::Process(clock) => clock.lock().ticks_at(now),
        };
        cpu_ticks_to_us(ticks)
    }
}

fn cpu_ticks_to_us(ticks: usize) -> usize {
    let frequency = get_accounting_clock_freq();
    (ticks / frequency)
        .saturating_mul(1_000_000)
        .saturating_add((ticks % frequency).saturating_mul(1_000_000) / frequency)
}

#[inline]
fn memory_set_read(lock: &RwLock<MemorySet>) -> RwLockReadGuard<'_, MemorySet> {
    #[cfg(target_arch = "riscv64")]
    {
        lock.read()
    }
    #[cfg(target_arch = "loongarch64")]
    loop {
        if let Some(guard) = lock.try_read() {
            return guard;
        }
        crate::arch::smp::poll_pending_ipi();
        core::hint::spin_loop();
    }
}

#[inline]
fn memory_set_write(lock: &RwLock<MemorySet>) -> RwLockWriteGuard<'_, MemorySet> {
    #[cfg(target_arch = "riscv64")]
    {
        lock.write()
    }
    #[cfg(target_arch = "loongarch64")]
    loop {
        if let Some(guard) = lock.try_write() {
            return guard;
        }
        crate::arch::smp::poll_pending_ipi();
        core::hint::spin_loop();
    }
}

/// 线程 tid 地址信息，用于 pthread 线程退出同步。
pub struct TidAddress {
    /// 当 CLONE_CHILD_SETTID 被设置时，新线程将其 TID 写入此地址。
    pub set_child_tid: Option<usize>,
    /// 线程退出时清零并做 futex wake 的用户空间地址。
    pub clear_child_tid: Option<usize>,
    /// set_robust_list 注册的 robust_list_head 地址。
    pub robust_list_head: Option<usize>,
    /// robust_list_head 结构长度。
    pub robust_list_len: usize,
}

impl TidAddress {
    pub fn new() -> Self {
        Self {
            set_child_tid: None,
            clear_child_tid: None,
            robust_list_head: None,
            robust_list_len: 0,
        }
    }
}

const RLIMIT_COUNT: usize = 16;
const RLIMIT_FSIZE: usize = 1;
const RLIMIT_STACK: usize = 3;
const RLIMIT_MEMLOCK: usize = 8;
pub const RLIMIT_SIGPENDING: usize = 11;
const DEFAULT_CPU_AFFINITY_MASK: usize = usize::MAX;
/// 没有 CPU 正在执行或尚未保存该任务的内核上下文。
///
/// 任务从 running context 切换到 per-CPU idle context 的过程中继续保留 owner。此窗口中
/// wakeup 可以把它放回全局 ready queue，但另一 CPU 尚不能恢复这份仍在保存中的 context。
pub const NO_CPU_OWNER: usize = usize::MAX;
const SCHED_OTHER: usize = 0;
const SCHED_FIFO: usize = 1;
const SCHED_RR: usize = 2;
const ROOT_CAP_MASK: usize = u32::MAX as usize;

pub const RLIMIT_NOFILE: usize = 7;

#[derive(Copy, Clone)]
struct LimitPair {
    cur: usize,
    max: usize,
}

struct ResourceLimits {
    limits: SpinLock<[LimitPair; RLIMIT_COUNT]>,
}

struct SchedState {
    nice: AtomicI32,
    policy: AtomicUsize,
    priority: AtomicI32,
    cpu_affinity_mask: AtomicUsize,
    reset_on_fork: AtomicBool,
}

impl SchedState {
    fn new() -> Self {
        Self {
            nice: AtomicI32::new(0),
            policy: AtomicUsize::new(SCHED_OTHER),
            priority: AtomicI32::new(0),
            cpu_affinity_mask: AtomicUsize::new(DEFAULT_CPU_AFFINITY_MASK),
            reset_on_fork: AtomicBool::new(false),
        }
    }

    fn from_parent(parent: &Self) -> Self {
        let mut nice = parent.nice();
        let mut policy = parent.policy();
        let mut priority = parent.priority();
        if parent.reset_on_fork() {
            if matches!(policy, SCHED_FIFO | SCHED_RR) {
                policy = SCHED_OTHER;
                priority = 0;
            }
            nice = nice.max(0);
        }

        Self {
            nice: AtomicI32::new(nice),
            policy: AtomicUsize::new(policy),
            priority: AtomicI32::new(priority),
            cpu_affinity_mask: AtomicUsize::new(parent.cpu_affinity_mask()),
            reset_on_fork: AtomicBool::new(false),
        }
    }

    fn nice(&self) -> i32 {
        self.nice.load(Ordering::Relaxed)
    }

    fn policy(&self) -> usize {
        self.policy.load(Ordering::Relaxed)
    }

    fn priority(&self) -> i32 {
        self.priority.load(Ordering::Relaxed)
    }

    fn cpu_affinity_mask(&self) -> usize {
        self.cpu_affinity_mask.load(Ordering::Relaxed)
    }

    fn reset_on_fork(&self) -> bool {
        self.reset_on_fork.load(Ordering::Relaxed)
    }

    fn set_nice(&self, nice: i32) {
        self.nice.store(nice.clamp(-20, 19), Ordering::Relaxed);
    }

    fn set_sched(&self, policy: usize, priority: i32, reset_on_fork: bool) {
        self.policy.store(policy, Ordering::Relaxed);
        self.priority.store(priority, Ordering::Relaxed);
        self.reset_on_fork.store(reset_on_fork, Ordering::Relaxed);
    }

    fn set_cpu_affinity_mask(&self, mask: usize) {
        self.cpu_affinity_mask.store(mask, Ordering::Relaxed);
    }
}

struct CapState {
    effective: AtomicUsize,
    permitted: AtomicUsize,
    inheritable: AtomicUsize,
}

impl CapState {
    fn root() -> Self {
        Self {
            effective: AtomicUsize::new(ROOT_CAP_MASK),
            permitted: AtomicUsize::new(ROOT_CAP_MASK),
            inheritable: AtomicUsize::new(0),
        }
    }

    fn from_parent(parent: &Self) -> Self {
        Self {
            effective: AtomicUsize::new(parent.effective()),
            permitted: AtomicUsize::new(parent.permitted()),
            inheritable: AtomicUsize::new(parent.inheritable()),
        }
    }

    fn effective(&self) -> usize {
        self.effective.load(Ordering::Relaxed)
    }

    fn permitted(&self) -> usize {
        self.permitted.load(Ordering::Relaxed)
    }

    fn inheritable(&self) -> usize {
        self.inheritable.load(Ordering::Relaxed)
    }

    fn has_cap(&self, cap: usize) -> bool {
        cap < usize::BITS as usize && (self.effective() & (1usize << cap)) != 0
    }

    fn set(&self, effective: usize, permitted: usize, inheritable: usize) {
        self.effective.store(effective, Ordering::Relaxed);
        self.permitted.store(permitted, Ordering::Relaxed);
        self.inheritable.store(inheritable, Ordering::Relaxed);
    }

    fn set_effective(&self, effective: usize) {
        self.effective.store(effective, Ordering::Relaxed);
    }

    fn set_permitted(&self, permitted: usize) {
        self.permitted.store(permitted, Ordering::Relaxed);
    }
}

impl ResourceLimits {
    fn new() -> Self {
        let mut limits = [LimitPair {
            cur: usize::MAX,
            max: usize::MAX,
        }; RLIMIT_COUNT];
        limits[RLIMIT_STACK] = LimitPair {
            cur: crate::config::USER_STACK_SIZE,
            max: usize::MAX,
        };
        Self {
            limits: SpinLock::new(limits),
        }
    }

    fn from_parent(parent: &Self) -> Self {
        Self {
            limits: SpinLock::new(*parent.limits.lock()),
        }
    }

    fn fsize_limit(&self) -> (usize, usize) {
        self.rlimit(RLIMIT_FSIZE).unwrap()
    }

    fn set_fsize_limit(&self, cur: usize, max: usize) -> SysResult {
        self.set_rlimit(RLIMIT_FSIZE, cur, max)
    }

    fn rlimit(&self, resource: usize) -> Option<(usize, usize)> {
        let limits = self.limits.lock();
        limits.get(resource).map(|limit| (limit.cur, limit.max))
    }

    fn set_rlimit(&self, resource: usize, cur: usize, max: usize) -> SysResult {
        if cur > max {
            return Err(Errno::EINVAL);
        }
        let mut limits = self.limits.lock();
        let limit = limits.get_mut(resource).ok_or(Errno::EINVAL)?;
        *limit = LimitPair { cur, max };
        Ok(())
    }
}

struct IntervalTimer {
    deadline_ms: AtomicUsize,
    interval_ms: AtomicUsize,
}

impl IntervalTimer {
    fn new() -> Self {
        Self {
            deadline_ms: AtomicUsize::new(0),
            interval_ms: AtomicUsize::new(0),
        }
    }
}

struct TaskTimers {
    timers: [IntervalTimer; 3],
}

impl TaskTimers {
    fn new() -> Self {
        Self {
            timers: [
                IntervalTimer::new(),
                IntervalTimer::new(),
                IntervalTimer::new(),
            ],
        }
    }

    fn fields(&self, which: usize) -> Option<(&AtomicUsize, &AtomicUsize, Sig)> {
        match which {
            0 => Some((
                &self.timers[0].deadline_ms,
                &self.timers[0].interval_ms,
                Sig::SIGALRM,
            )),
            1 => Some((
                &self.timers[1].deadline_ms,
                &self.timers[1].interval_ms,
                Sig::SIGVTALRM,
            )),
            2 => Some((
                &self.timers[2].deadline_ms,
                &self.timers[2].interval_ms,
                Sig::SIGPROF,
            )),
            _ => None,
        }
    }
}

pub struct NetNamespace {
    loopback_tag: AtomicUsize,
}

impl NetNamespace {
    const DEFAULT_LOOPBACK_TAG: usize = 0;

    fn new() -> Self {
        Self {
            loopback_tag: AtomicUsize::new(Self::DEFAULT_LOOPBACK_TAG),
        }
    }

    pub fn loopback_tag(&self) -> usize {
        self.loopback_tag.load(Ordering::Relaxed)
    }

    pub fn set_loopback_tag(&self, value: usize) {
        self.loopback_tag.store(value, Ordering::Relaxed);
    }

    pub fn default_loopback_tag() -> usize {
        Self::DEFAULT_LOOPBACK_TAG
    }
}

struct TaskInner {
    tid_address: TidAddress,
    sig_context_addr: usize,
    sigsuspend_saved_mask: Option<SigSet>,
}

impl TaskInner {
    fn new() -> Self {
        Self {
            tid_address: TidAddress::new(),
            sig_context_addr: 0,
            sigsuspend_saved_mask: None,
        }
    }
}

/// 任务控制块——此处的任务是对一定资源和某个程序的抽象表述
#[repr(C)]
pub struct TaskControlBlock {
    // 固定数据
    kernel_stack: KernelStack, // 对于当前实现，确保 `TaskControlBlock` 的第一个字段为内核栈

    // 基本数据
    tid: RwLock<TidHandle>,
    process: Arc<ProcessState>,
    tgid: AtomicUsize,
    pgid: AtomicUsize,
    sid: AtomicUsize,
    uid: AtomicUsize,
    euid: AtomicUsize,
    suid: AtomicUsize,
    gid: AtomicUsize,
    egid: AtomicUsize,
    sgid: AtomicUsize,
    fsuid: AtomicUsize,
    fsgid: AtomicUsize,
    supplementary_groups: SpinLock<Vec<usize>>,
    umask: AtomicUsize,
    sched: SchedState,
    caps: CapState,
    thread_group: Arc<SpinLock<ThreadGroup>>,
    group_exiting: Arc<AtomicBool>,
    /// exec/group teardown 在分离远端 sibling 前设置。若旧 CPU 仍在完成 context handoff，
    /// 该标记阻止此 context 再次变为可 claim。
    terminate_requested: AtomicBool,
    task_status: SpinLock<TaskStatus>,
    cpu_owner: AtomicUsize,
    // CLONE_VFORK 在普通 parent/child 关系之外还有独立同步边：child 成功 exec 或 exit 时
    // parent 必须恢复。保持一次性，避免 exit 的普通 SIGCHLD 唤醒把 parent 重复入队。
    vfork_parent: SpinLock<Option<Weak<TaskControlBlock>>>,
    // task_context: TaskContext, // 注意任务上下文的处理

    // 内存管理
    // 每个 task 拥有一个可替换的地址空间 handle。CLONE_VM 复制内部 Arc；exec 只替换调用者
    // 自己的 handle，不能覆盖仍被其 vfork/clone parent 使用的 MemorySet。
    memory_set: SpinNoIrqLock<Arc<RwLock<MemorySet>>>,

    // 文件系统
    fd_table: SpinLock<Arc<FdTable>>,
    cwd: Arc<SpinLock<Arc<Path>>>,
    root: Arc<SpinLock<Arc<Path>>>,
    exe_path: Arc<SpinLock<String>>,
    limits: Arc<ResourceLimits>,
    net_ns: Arc<NetNamespace>,

    //信号
    sig_pending: SpinLock<SigPending>, // 本线程的信号队列 + 掩码（独享）
    sig_stack: SpinLock<SignalStack>,  // 本线程的备用信号栈（独享）
    sig_handler: Arc<SpinLock<SigHandler>>, // 线程组共享的 handler 注册表（共享）

    // 线程同步
    inner: SpinLock<TaskInner>,
    // ===== 新增：可中断状态标记 =====
    // 标记当前线程是否处于"可被信号中断"的阻塞中（futex_wait / sigtimedwait / wait4）
    interruptible: AtomicBool,
    // rt_sigtimedwait 等待这些（通常已被 mask）signal 时为非零。signal 投递用它选择并唤醒
    // 真正 waiter，而不是把目标 signal 转换成 EINTR。
    sigtimedwait_mask: AtomicU64,
    // wait4/waitid 可由线程组中任意线程执行；子进程状态变化时需要
    // 唤醒真正的等待者，而不能只唤醒进程组长。
    waiting_for_child: AtomicBool,
    // 信号中断标记：当线程在 interruptible 状态下被信号唤醒时置为 true
    interrupted: AtomicBool,
    itimers: Arc<TaskTimers>,
    personality: AtomicUsize,
    thread_cpu_clock: Arc<SpinNoIrqLock<ThreadCpuClock>>,
    process_cpu_clock: Arc<SpinNoIrqLock<ProcessCpuClock>>,
    cpu_in_user: AtomicBool,
    thread_resource_usage: super::process::ResourceUsageCounters,
}

impl core::fmt::Debug for TaskControlBlock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Task")
            .field("tid", &self.tid())
            .field("tgid", &self.tgid())
            .finish()
    }
}

impl TaskControlBlock {
    /// 全零初始化
    pub fn zero_init() -> Self {
        Self {
            // 固定数据
            kernel_stack: KernelStack::zero_init(),

            // 基本数据
            tid: RwLock::new(TidHandle(0)),
            process: ProcessState::new(0, 0, 0, 0, 0, 0, false),
            tgid: AtomicUsize::new(0),
            pgid: AtomicUsize::new(0),
            sid: AtomicUsize::new(0),
            uid: AtomicUsize::new(0),
            euid: AtomicUsize::new(0),
            suid: AtomicUsize::new(0),
            gid: AtomicUsize::new(0),
            egid: AtomicUsize::new(0),
            sgid: AtomicUsize::new(0),
            fsuid: AtomicUsize::new(0),
            fsgid: AtomicUsize::new(0),
            supplementary_groups: SpinLock::new(Vec::new()),
            umask: AtomicUsize::new(0o022),
            sched: SchedState::new(),
            caps: CapState::root(),
            thread_group: Arc::new(SpinLock::new(ThreadGroup::new())),
            group_exiting: Arc::new(AtomicBool::new(false)),
            terminate_requested: AtomicBool::new(false),
            task_status: SpinLock::new(TaskStatus::Ready),
            cpu_owner: AtomicUsize::new(NO_CPU_OWNER),
            vfork_parent: SpinLock::new(None),
            // task_context: TaskContext, // 注意任务上下文的处理

            // 内存管理
            memory_set: SpinNoIrqLock::new(Arc::new(RwLock::new(MemorySet::new()))),

            // 文件系统
            fd_table: SpinLock::new(FdTable::new()),
            cwd: Arc::new(SpinLock::new(Path::zero_init())),
            root: Arc::new(SpinLock::new(Path::zero_init())),
            exe_path: Arc::new(SpinLock::new(String::new())),
            limits: Arc::new(ResourceLimits::new()),
            net_ns: Arc::new(NetNamespace::new()),

            //信号
            sig_pending: SpinLock::new(SigPending::new()),
            sig_stack: SpinLock::new(SignalStack::default()),
            sig_handler: Arc::new(SpinLock::new(SigHandler::new())),

            // 线程同步
            inner: SpinLock::new(TaskInner::new()),

            // 可中断状态
            interruptible: AtomicBool::new(false),
            sigtimedwait_mask: AtomicU64::new(0),
            waiting_for_child: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
            itimers: Arc::new(TaskTimers::new()),
            personality: AtomicUsize::new(0),
            thread_cpu_clock: Arc::new(SpinNoIrqLock::new(ThreadCpuClock::default())),
            process_cpu_clock: Arc::new(SpinNoIrqLock::new(ProcessCpuClock::new())),
            cpu_in_user: AtomicBool::new(true),
            thread_resource_usage: super::process::ResourceUsageCounters::new(),
        }
    }

    /// 从内嵌 ELF 创建系统中的第一个用户任务。
    ///
    /// 该启动专用路径同时创建 pid/tgid 身份、ProcessState、内核栈、用户地址空间、根/cwd、
    /// fd 表、信号与 CPU 记账对象，并在栈顶布置 TrapContext 和首次调度 TaskContext。
    /// 内核栈必须先于用户页表复制建立，使用户 root 能看到完整的内核半区拓扑。
    ///
    /// 本函数使用不可恢复分配并返回完整 Arc；只有所有字段和上下文初始化完成后，调用方
    /// 才能把 init 加入任务/进程管理器和调度队列。普通 fork/clone 不得复用该路径。
    pub fn init(elf_data: &[u8]) -> Arc<Self> {
        let tid: TidHandle = tid_alloc();
        let tgid = tid.0;
        let process = ProcessState::new(tgid, tgid, tgid, 0, 0, 0, true);
        // 创建地址空间会拷贝内核页表，先创建内核栈生成页表映射，以保证任务切换后能正确访问内核栈
        let mut kernel_stack =
            KernelStack::new(&tid).expect("failed to allocate init kernel stack");
        let (memory_set, token, user_sp, entry_point, _aux_vec) =
            MemorySet::from_elf_data(elf_data);

        let mut kernel_stack_top = kernel_stack.get_top(); // 由于栈是新建的，栈顶就是栈顶边界
                                                           // 在栈上存储异常上下文，该数据不会从栈中弹出，固定位于栈最高位置
        kernel_stack_top -= core::mem::size_of::<TrapContext>();
        let trap_cx_ptr = kernel_stack_top as *mut TrapContext;
        // 在栈上设置任务上下文，使任务可被正常切换
        kernel_stack_top -= core::mem::size_of::<TaskContext>();
        let task_cx_ptr = kernel_stack_top as *mut TaskContext;
        // 重新设置栈顶指针
        kernel_stack.set_top(kernel_stack_top);

        // 创建进程控制块
        let root = init_root_fs();
        let task_ctrl_block = Arc::new(Self {
            // 固定数据
            kernel_stack,

            // 基本数据
            tid: RwLock::new(tid),
            process: process.clone(),
            tgid: AtomicUsize::new(tgid),
            pgid: AtomicUsize::new(tgid),
            sid: AtomicUsize::new(tgid),
            uid: AtomicUsize::new(0),
            euid: AtomicUsize::new(0),
            suid: AtomicUsize::new(0),
            gid: AtomicUsize::new(0),
            egid: AtomicUsize::new(0),
            sgid: AtomicUsize::new(0),
            fsuid: AtomicUsize::new(0),
            fsgid: AtomicUsize::new(0),
            supplementary_groups: SpinLock::new(Vec::new()),
            umask: AtomicUsize::new(0o022),
            sched: SchedState::new(),
            caps: CapState::root(),
            thread_group: Arc::new(SpinLock::new(ThreadGroup::new())),
            group_exiting: Arc::new(AtomicBool::new(false)),
            terminate_requested: AtomicBool::new(false),
            task_status: SpinLock::new(TaskStatus::Ready),
            cpu_owner: AtomicUsize::new(NO_CPU_OWNER),
            vfork_parent: SpinLock::new(None),

            // 内存管理
            memory_set: SpinNoIrqLock::new(Arc::new(RwLock::new(memory_set))),

            // 文件系统
            fd_table: SpinLock::new(FdTable::new()),
            cwd: Arc::new(SpinLock::new(root.clone())),
            root: Arc::new(SpinLock::new(root)),
            exe_path: Arc::new(SpinLock::new(String::new())),
            limits: Arc::new(ResourceLimits::new()),
            net_ns: Arc::new(NetNamespace::new()),

            //信号
            sig_pending: SpinLock::new(SigPending::new()),
            sig_stack: SpinLock::new(SignalStack::default()),
            sig_handler: Arc::new(SpinLock::new(SigHandler::new())),

            // 线程同步
            inner: SpinLock::new(TaskInner::new()),

            // 可中断状态
            interruptible: AtomicBool::new(false),
            sigtimedwait_mask: AtomicU64::new(0),
            waiting_for_child: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
            itimers: Arc::new(TaskTimers::new()),
            personality: AtomicUsize::new(0),
            thread_cpu_clock: Arc::new(SpinNoIrqLock::new(ThreadCpuClock::default())),
            process_cpu_clock: Arc::new(SpinNoIrqLock::new(ProcessCpuClock::new())),
            cpu_in_user: AtomicBool::new(true),
            thread_resource_usage: super::process::ResourceUsageCounters::new(),
        });

        // 在线程组中添加该线程
        task_ctrl_block
            .thread_group
            .lock()
            .add(task_ctrl_block.clone());
        process.add_member(&task_ctrl_block);
        PROCESS_MANAGER.add(&process);

        // 初始化内核栈上的异常上下文
        let trap_context = TrapContext::init_app_context(entry_point, user_sp, 0, 0, 0, 0, false);
        // 初始化任务上下文
        let mut task_context = TaskContext::app_init_task_context(token);
        task_context.set_tp(Arc::as_ptr(&task_ctrl_block) as usize);

        // 修改内核栈中上下文数据
        unsafe {
            trap_cx_ptr.write(trap_context);
            task_cx_ptr.write(task_context);
        }

        // 在任务管理器中添加线程号到线程的映射
        TASK_MANAGER.add(&task_ctrl_block);

        task_ctrl_block
    }

    /// 根据 Linux `clone` 标志复制当前任务，创建线程或新进程。
    ///
    /// 该函数同时负责内核栈/陷入上下文、地址空间、文件表、信号处理器、线程组、
    /// 进程身份、父子关系以及 CPU 记账对象的所有权组合。`CLONE_VM`、`CLONE_FILES`、
    /// `CLONE_SIGHAND` 等标志决定共享 Arc 还是建立独立副本；没有 `CLONE_VM` 时通过
    /// COW 创建子地址空间。新任务只有在所有可失败分配完成后才加入线程组、
    /// `PROCESS_MANAGER` 和 `TASK_MANAGER`，避免外部观察到半初始化任务。
    ///
    /// 返回的新任务处于 Ready 但尚未执行；用户态 child-tid、TLS、父任务阻塞等 ABI
    /// 提交由 `sys_clone` 在本函数成功后完成。
    pub fn clone_(self: &Arc<Self>, flags: CloneFlags) -> SysResult<Arc<Self>> {
        let tid = tid_alloc();

        // 克隆内核栈
        let mut kernel_stack = KernelStack::new(&tid)?;
        let mut kernel_stack_top = kernel_stack.get_top(); // 由于栈是新建的，栈顶就是栈顶边界
        kernel_stack_top -= core::mem::size_of::<TrapContext>();
        self.clone_trap_cx(kernel_stack_top);
        kernel_stack_top -= core::mem::size_of::<TaskContext>();
        // 注意这里只修改了栈指针但没有修改栈上的任务上下文，这需要在创建完任务控制块后再调用相关函数
        kernel_stack.set_top(kernel_stack_top);

        let is_thread = flags.contains(CloneFlags::CLONE_THREAD);

        let process_leader = self
            .op_thread_group(|tg| tg.iter().find(|task| task.tid() == self.tgid()))
            .unwrap_or_else(|| self.clone());

        // 创建线程或是进程
        let (
            tgid,
            pgid,
            sid,
            thread_group,
            group_exiting,
            cwd,
            root,
            exe_path,
            parent_for_child,
            process,
        ) = if is_thread {
            // 创建线程，属于同一线程组
            (
                self.tgid(),
                self.pgid(),
                self.sid(),
                self.thread_group.clone(),
                self.group_exiting.clone(),
                self.cwd.clone(),
                self.root.clone(),
                self.exe_path.clone(),
                None,
                self.process.clone(),
            )
        } else {
            let parent_for_child = if flags.contains(CloneFlags::CLONE_PARENT) {
                self.process
                    .parent()
                    .and_then(|parent| parent.signal_target())
                    .unwrap_or_else(|| INITPROC.clone())
            } else {
                process_leader.clone()
            };
            // 创建进程
            (
                tid.0,
                self.pgid(),
                self.sid(),
                Arc::new(SpinLock::new(ThreadGroup::new())),
                Arc::new(AtomicBool::new(false)),
                Arc::new(SpinLock::new(Path::from_existed_user(&self.cwd()))),
                Arc::new(SpinLock::new(Path::from_existed_user(&self.root()))),
                Arc::new(SpinLock::new(self.exe_path())),
                Some(parent_for_child),
                ProcessState::new(
                    tid.0,
                    self.pgid(),
                    self.sid(),
                    self.uid(),
                    self.euid(),
                    self.suid(),
                    self.process.has_controlling_tty(),
                ),
            )
        };

        // 是否与父线程共享地址空间
        let parent_memory_set = self.memory_set_arc();
        let memory_set = if flags.share_user_vm() {
            parent_memory_set
        } else {
            Arc::new(RwLock::new(MemorySet::from_existed_user(
                &mut memory_set_write(&parent_memory_set),
            )?))
        };

        // 是否与父线程共享文件数据
        let fd_table = if is_thread || flags.contains(CloneFlags::CLONE_FILES) {
            self.fd_table.lock().clone()
        } else {
            FdTable::from_existed_user(&self.fd_table.lock())
        };
        let limits = if is_thread {
            self.limits.clone()
        } else {
            Arc::new(ResourceLimits::from_parent(&self.limits))
        };
        let itimers = if is_thread {
            self.itimers.clone()
        } else {
            Arc::new(TaskTimers::new())
        };
        let process_cpu_clock = if is_thread {
            self.process_cpu_clock.clone()
        } else {
            Arc::new(SpinNoIrqLock::new(ProcessCpuClock::new()))
        };
        let net_ns = if flags.contains(CloneFlags::CLONE_NEWNET) {
            Arc::new(NetNamespace::new())
        } else {
            self.net_ns.clone()
        };
        let current_sig_mask = self.op_sig_pending(|pending| pending.mask);
        let (sig_handler, sig_pending, sig_stack) = if is_thread {
            (
                self.sig_handler.clone(), // 共享同一张 handler 表
                SpinLock::new(SigPending::with_mask(current_sig_mask)), // 自己的队列，继承当前 mask
                SpinLock::new(SignalStack::default()), // 自己的栈
            )
        } else {
            (
                Arc::new(SpinLock::new(
                    self.op_sig_handler(|handler| handler.clone()),
                )),
                SpinLock::new(SigPending::with_mask(current_sig_mask)),
                SpinLock::new(*self.sig_stack.lock()),
            )
        };
        let task_ctrl_block = Arc::new(TaskControlBlock {
            // 固定数据
            kernel_stack,

            // 基本数据
            tid: RwLock::new(tid),
            process: process.clone(),
            tgid: AtomicUsize::new(tgid),
            pgid: AtomicUsize::new(pgid),
            sid: AtomicUsize::new(sid),
            uid: AtomicUsize::new(self.uid()),
            euid: AtomicUsize::new(self.euid()),
            suid: AtomicUsize::new(self.suid()),
            gid: AtomicUsize::new(self.gid()),
            egid: AtomicUsize::new(self.egid()),
            sgid: AtomicUsize::new(self.sgid()),
            fsuid: AtomicUsize::new(self.fsuid()),
            fsgid: AtomicUsize::new(self.fsgid()),
            supplementary_groups: SpinLock::new(self.supplementary_groups()),
            umask: AtomicUsize::new(self.umask()),
            sched: SchedState::from_parent(&self.sched),
            caps: CapState::from_parent(&self.caps),
            thread_group,
            group_exiting,
            terminate_requested: AtomicBool::new(false),
            task_status: SpinLock::new(TaskStatus::Ready),
            cpu_owner: AtomicUsize::new(NO_CPU_OWNER),
            vfork_parent: SpinLock::new(None),

            // 内存管理
            memory_set: SpinNoIrqLock::new(memory_set),

            // 文件系统
            fd_table: SpinLock::new(fd_table),
            cwd,
            root,
            exe_path,
            limits,
            net_ns,

            // 信号
            sig_pending,
            sig_stack,
            sig_handler,

            // 线程同步
            inner: SpinLock::new(TaskInner::new()),

            // 可中断状态
            interruptible: AtomicBool::new(false),
            sigtimedwait_mask: AtomicU64::new(0),
            waiting_for_child: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
            itimers,
            personality: AtomicUsize::new(self.personality()),
            thread_cpu_clock: Arc::new(SpinNoIrqLock::new(ThreadCpuClock::default())),
            process_cpu_clock,
            cpu_in_user: AtomicBool::new(true),
            thread_resource_usage: super::process::ResourceUsageCounters::new(),
        });

        // 修改任务异常上下文
        task_ctrl_block.write_task_cx(kernel_stack_top);

        // 只有新进程进入 children；同线程组内的新线程不由 wait4 回收。
        if let Some(parent) = parent_for_child {
            parent.add_child(task_ctrl_block.clone());
        }
        // 在线程组中添加线程
        task_ctrl_block.op_thread_group_mut(|tg| tg.add(task_ctrl_block.clone()));
        process.add_member(&task_ctrl_block);
        PROCESS_MANAGER.add(&process);

        // 在任务管理器中添加线程号到线程的映射
        TASK_MANAGER.add(&task_ctrl_block);

        Ok(task_ctrl_block)
    }

    /// 载入可执行程序，主要修改地址空间、用户栈、异常上下文等数据
    ///
    /// 将命令行参数个数 `argc` 作为返回值，考虑到系统调用异常时会统一修改 `a0` 寄存器
    pub fn execve(
        self: &Arc<Self>,
        exe_path: String,
        elf_data: &[u8],
        mut args: Vec<String>,
        envs: Vec<String>,
        linux_abi: bool,
    ) -> SysResult<usize> {
        if args.is_empty() {
            args.push(String::new());
        }

        let loaded = MemorySet::try_from_elf_data(elf_data)?;
        self.install_exec_image(exe_path, loaded, args, envs, linux_abi)
    }

    pub fn execve_file(
        self: &Arc<Self>,
        exe_path: String,
        file: Arc<dyn FileOp>,
        mut args: Vec<String>,
        envs: Vec<String>,
        linux_abi: bool,
    ) -> SysResult<usize> {
        if args.is_empty() {
            args.push(String::new());
        }

        let loaded = MemorySet::try_from_elf_file(file)?;
        self.install_exec_image(exe_path, loaded, args, envs, linux_abi)
    }

    /// 将已经完整解析的 ELF 地址空间提交为当前进程的新执行映像。
    ///
    /// 提交前先在临时 `MemorySet` 中构造 argv、envp、auxv 和用户栈；这些步骤失败时旧映像
    /// 仍保持完整。`begin_exec` 成功后进入不可回滚阶段：停止同线程组其他成员、处理旧映像的
    /// robust futex/clear-child-tid、替换并激活页表、释放旧 SysV SHM 附加关系，然后重建
    /// trap context。最后解除共享 fd 表、应用 close-on-exec，并复位不能跨 exec 保留的信号状态。
    ///
    /// 调用者必须保证 `loaded` 尚未被其他任务使用。成功返回 argc；从提交点开始不得再返回
    /// 会让用户态继续使用半拆除旧映像的错误。
    fn install_exec_image(
        self: &Arc<Self>,
        exe_path: String,
        loaded: (MemorySet, usize, usize, usize, Vec<AuxHeader>),
        args: Vec<String>,
        envs: Vec<String>,
        linux_abi: bool,
    ) -> SysResult<usize> {
        let (mut memory_set, _token, mut user_sp, entry_point, aux_vec) = loaded;

        /* ===== 修改用户栈数据 ===== */
        let (argv_base, envp_base, auxv_base, stack_top) = init_user_stack(
            &mut memory_set,
            exe_path.as_str(),
            args.as_slice(),
            envs.as_slice(),
            aux_vec,
            &mut user_sp,
        )?;

        // 所有可能失败的 image 准备均已完成。从这里开始 exec 拥有进程转换，可以 quiesce
        // sibling，且不再回到已部分拆除的旧 image。
        if !self.process.begin_exec() {
            return Err(Errno::EAGAIN);
        }

        // 线程私有 robust-list 与 clear_child_tid 地址属于旧 image。趁旧地址空间仍安装时拆除
        // sibling thread；若在替换 MemorySet 后执行，陈旧线程 metadata 会写入新加载的程序。
        self.close_other_threads_for_exec();
        self.adopt_process_tgid_for_exec();

        // 存活线程也携带旧 image 的线程 metadata。在旧 mapping 仍安装时完成 robust-futex
        // recovery，再丢弃所有不能跨 exec 保留的用户地址。
        exit_robust_list(self);
        self.reset_tid_address_for_exec();

        /* ===== 修改地址空间 ===== */
        let new_memory_set = Arc::new(RwLock::new(memory_set));
        let old_memory_set = {
            let mut memory_set_handle = self.memory_set.lock();
            core::mem::replace(&mut *memory_set_handle, new_memory_set.clone())
        };
        let old_shm_attach_ids = memory_set_read(&old_memory_set).shm_attach_ids();
        // 刷新页表，由于应用程序通过异常进入，在异常返回时不会刷新页表
        // 为了程序返回后看到的地址空间为自身而非父任务的地址空间，需要主动刷新页表
        memory_set_read(&new_memory_set).activate();
        memory_set_read(&old_memory_set).clear_current_hart_active();
        drop(old_memory_set);
        crate::syscall::ipc::release_shm_attachments(
            old_shm_attach_ids.as_slice(),
            self.tgid() as i32,
        );

        /* ===== 修改异常上下文 ===== */
        let argc = args.len();
        let trap_cx = self.get_trap_cx();
        *trap_cx = TrapContext::init_app_context(
            entry_point,
            stack_top,
            argc,
            argv_base,
            envp_base,
            auxv_base,
            linux_abi,
        );

        // 记录可执行文件路径，供 /proc/self/exe 使用。到这里 exec 已经完成了
        // 新地址空间和用户栈的关键构造，父进程不应再能修改它的 pgid。
        self.set_exe_path(exe_path);
        self.process.mark_exec();

        /* ===== 修改文件描述符表 ===== */
        // Linux execve 在应用 close-on-exec 前总会解除 CLONE_FILES 共享。否则 exec 后的 child
        // 仍能修改 parent 的 fd table，pipe endpoint 也会与 parent 一样长寿，使捕获的
        // stdout/stderr 永远到不了 EOF。
        self.unshare_fd_table_for_exec();
        self.fd_table.lock().close_on_exec();

        /* ===== 修改信号处理 ===== */
        self.op_sig_handler_mut(|handler| handler.reset_user_handlers_for_exec());
        *self.sig_stack.lock() = SignalStack::default();

        self.process.finish_exec();

        // Linux vfork 在 child 安装新 image 后恢复 parent；必须等所有 exec 可见状态就绪后再做。
        self.release_vfork_parent();

        Ok(argc)
    }
}

impl TaskControlBlock {
    /* ======= 获取内部数据 ====== */
    /// 线程号
    pub fn tid(&self) -> usize {
        self.tid.read().0
    }
    /// 线程组号
    pub fn tgid(&self) -> usize {
        let tgid = self.process.tgid();
        debug_assert_eq!(self.tgid.load(Ordering::Relaxed), tgid);
        tgid
    }
    pub fn process(&self) -> Arc<ProcessState> {
        self.process.clone()
    }
    pub fn pgid(&self) -> usize {
        let pgid = self.process.pgid();
        debug_assert_eq!(self.pgid.load(Ordering::Relaxed), pgid);
        pgid
    }
    pub fn sid(&self) -> usize {
        let sid = self.process.sid();
        debug_assert_eq!(self.sid.load(Ordering::Relaxed), sid);
        sid
    }
    pub fn uid(&self) -> usize {
        self.process.uid()
    }
    pub fn euid(&self) -> usize {
        self.process.euid()
    }
    pub fn suid(&self) -> usize {
        self.process.suid()
    }
    pub fn gid(&self) -> usize {
        self.gid.load(Ordering::Relaxed)
    }
    pub fn egid(&self) -> usize {
        self.egid.load(Ordering::Relaxed)
    }
    pub fn sgid(&self) -> usize {
        self.sgid.load(Ordering::Relaxed)
    }
    pub fn fsuid(&self) -> usize {
        self.fsuid.load(Ordering::Relaxed)
    }
    pub fn fsgid(&self) -> usize {
        self.fsgid.load(Ordering::Relaxed)
    }
    pub fn supplementary_groups(&self) -> Vec<usize> {
        self.supplementary_groups.lock().clone()
    }
    pub fn in_group(&self, gid: usize) -> bool {
        self.fsgid() == gid || self.supplementary_groups.lock().contains(&gid)
    }
    pub fn nice(&self) -> i32 {
        self.sched.nice()
    }
    pub fn sched_policy(&self) -> usize {
        self.sched.policy()
    }
    pub fn sched_priority(&self) -> i32 {
        self.sched.priority()
    }
    pub fn cpu_affinity_mask(&self) -> usize {
        self.sched.cpu_affinity_mask()
    }

    /// 这个 ready task 是否能由 `cpu` claim，且不违反 affinity 或 context-save owner handoff。
    ///
    /// scheduler 持有全局 ready-queue 锁时调用，因此从此次检查到
    /// `try_claim_running_on_cpu()` 之间，另一 CPU 不能 claim 同一 queued task。outgoing
    /// context 到达 idle 后，owner 可并发地从本 CPU 变为 `NO_CPU_OWNER`；过早返回 false
    /// 只会让任务留在队列中，等待下一次 fetch。
    pub fn can_be_claimed_on_cpu(&self, cpu: usize) -> bool {
        cpu < usize::BITS as usize
            && self.cpu_affinity_mask() & (1usize << cpu) != 0
            && !self.terminate_requested.load(Ordering::Acquire)
            && self.cpu_owner.load(Ordering::Acquire) == NO_CPU_OWNER
    }
    pub fn cap_effective(&self) -> usize {
        self.caps.effective()
    }
    pub fn cap_permitted(&self) -> usize {
        self.caps.permitted()
    }
    pub fn cap_inheritable(&self) -> usize {
        self.caps.inheritable()
    }
    pub fn has_cap(&self, cap: usize) -> bool {
        self.caps.has_cap(cap)
    }
    pub fn umask(&self) -> usize {
        self.umask.load(Ordering::Relaxed)
    }
    pub fn net_ns(&self) -> Arc<NetNamespace> {
        self.net_ns.clone()
    }
    pub fn status(&self) -> TaskStatus {
        self.task_status.lock().clone()
    }

    /// 为一个 CPU claim ready context。scheduler 锁串行化出队；owner CAS 额外串行化“任务
    /// 已被唤醒，但旧 CPU 尚未保存完 context”的短窗口。
    pub fn try_claim_running_on_cpu(&self, cpu: usize) -> bool {
        let mut status = self.task_status.lock();
        if *status != TaskStatus::Ready || self.terminate_requested.load(Ordering::Acquire) {
            return false;
        }
        if self
            .cpu_owner
            .compare_exchange(NO_CPU_OWNER, cpu, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        *status = TaskStatus::Running;
        true
    }

    /// 释放拥有该任务 saved context 的 CPU。只能在 `__switch` 已离开该任务后，从本地 idle
    /// context 执行。
    pub fn release_cpu_owner(&self, cpu: usize) {
        assert_eq!(
            self.cpu_owner.load(Ordering::Acquire),
            cpu,
            "task {} released by a CPU that does not own it",
            self.tid()
        );
        self.cpu_owner.store(NO_CPU_OWNER, Ordering::Release);
    }

    pub fn assert_cpu_owner(&self, cpu: usize) {
        assert_eq!(
            self.cpu_owner.load(Ordering::Acquire),
            cpu,
            "task {} is running on a CPU that did not claim it",
            self.tid()
        );
    }

    /// 是否仍有 CPU 能执行该任务的 saved context。只有该 CPU 已切回 per-CPU idle context
    /// 后才发布 `NO_CPU_OWNER`。
    pub fn has_cpu_owner(&self) -> bool {
        self.cpu_owner.load(Ordering::Acquire) != NO_CPU_OWNER
    }

    pub fn request_termination(&self) {
        self.terminate_requested.store(true, Ordering::Release);
        self.set_exited();
    }

    pub fn termination_requested(&self) -> bool {
        self.terminate_requested.load(Ordering::Acquire)
    }

    pub fn cwd(&self) -> Arc<Path> {
        self.cwd.lock().clone()
    }
    pub fn root(&self) -> Arc<Path> {
        self.root.lock().clone()
    }
    pub fn exe_path(&self) -> String {
        self.exe_path.lock().clone()
    }
    pub fn exit_code(&self) -> i32 {
        (self.process.wait_status() >> 8) & 0xff
    }
    pub fn wait_status(&self) -> i32 {
        self.process.wait_status()
    }
    /// 获取用户任务页表的页表基址寄存器值
    pub fn get_user_token(&self) -> usize {
        memory_set_read(&self.memory_set_arc()).token()
    }

    /// 在恢复本任务 context 前发布当前 hart 正在使用其地址空间。
    pub fn mark_memory_set_current_hart_active(&self) {
        memory_set_read(&self.memory_set_arc()).mark_current_hart_active();
    }

    /// 只可在本 hart 已经切回 idle/kernel 页表后调用。
    pub fn clear_memory_set_current_hart_active(&self) {
        memory_set_read(&self.memory_set_arc()).clear_current_hart_active();
    }
    // 任务状态判断
    pub fn is_running(&self) -> bool {
        self.status() == TaskStatus::Running
    }
    pub fn is_ready(&self) -> bool {
        self.status() == TaskStatus::Ready
    }
    pub fn is_blocked(&self) -> bool {
        self.status() == TaskStatus::Blocked
    }
    pub fn is_stopped(&self) -> bool {
        self.status() == TaskStatus::Stopped
    }
    // ===== 可中断状态管理 =====

    /// 进入可中断的阻塞前调用
    pub fn set_interruptible(&self, val: bool) {
        self.interruptible.store(val, Ordering::Relaxed);
    }

    pub fn set_sigtimedwait_mask(&self, mask: SigSet) {
        self.sigtimedwait_mask.store(mask.bits(), Ordering::Release);
    }

    pub fn clear_sigtimedwait_mask(&self) {
        self.sigtimedwait_mask.store(0, Ordering::Release);
    }

    fn is_sigtimedwait_waiter_for(&self, sig: Sig) -> bool {
        let bit = 1u64 << (sig.raw() - 1);
        self.sigtimedwait_mask.load(Ordering::Acquire) & bit != 0
    }

    /// 是否处于可中断状态
    fn is_interruptible(&self) -> bool {
        self.interruptible.load(Ordering::Relaxed)
    }

    /// 信号中断唤醒后检查
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }

    /// 清除中断标记（处理完 EINTR 后调用）
    pub fn clear_interrupted(&self) {
        self.interrupted.store(false, Ordering::Relaxed);
    }

    /// 发布“存在可投递 signal”的唤醒提示，然后重新验证。
    ///
    /// waiter 可能在 sender 第一次 `check_signal_interrupt()` 与此次 store 之间观察并消费
    /// queued signal。若没有第二次检查，这个迟到 store 会泄漏到下一次 blocking syscall，
    /// 在已无可投递 signal 时伪造 EINTR。
    fn mark_signal_interrupted(&self) -> bool {
        self.interrupted.store(true, Ordering::Relaxed);
        if self.check_signal_interrupt() {
            true
        } else {
            self.interrupted.store(false, Ordering::Relaxed);
            false
        }
    }

    /// ★ 核心判断：当前线程是否应该被 pending 信号中断
    /// 条件：
    ///   1. 线程处于可中断状态 (interruptible == true)
    ///   2. 存在 pending 信号没有被掩码屏蔽
    ///   3. 该信号的处理程序不是 SIG_IGN（sa_handler != 1）
    pub fn check_signal_interrupt(&self) -> bool {
        if !self.is_interruptible() {
            return false;
        }
        // find_signal 已经帮我们跳过了被 mask 屏蔽的信号
        if let Some(sig) = self.find_pending_signal() {
            let action = self.op_sig_handler(|handler| handler.get(sig));
            // sa_handler == SIG_IGN(1) → 信号被显式忽略，不需要打断
            action.sa_handler != SIG_IGN
                && (action.sa_handler != 0 || ActionType::default(sig) != ActionType::Ignore)
        } else {
            false
        }
    }
    pub fn is_exited(&self) -> bool {
        self.status() == TaskStatus::Exited
    }
    pub fn is_process_leader(&self) -> bool {
        self.tid() == self.tgid()
    }
    pub fn did_exec(&self) -> bool {
        self.process.did_exec()
    }
    /* ======= 设置内部数据 ====== */
    pub fn set_tgid(&self, tgid: usize) {
        debug_assert_eq!(self.process.tgid(), tgid);
        self.tgid.swap(tgid, Ordering::Relaxed);
    }
    pub fn set_pgid(&self, pgid: usize) {
        self.process.set_pgid(pgid);
        self.pgid.store(pgid, Ordering::Relaxed);
    }
    pub fn set_sid(&self, sid: usize) {
        self.process.set_sid(sid);
        self.sid.store(sid, Ordering::Relaxed);
    }
    pub fn set_uid_triplet(&self, uid: usize, euid: usize, suid: usize) {
        let old_euid = self.euid();
        self.process.set_uid_triplet(uid, euid, suid);
        self.uid.store(uid, Ordering::Relaxed);
        self.euid.store(euid, Ordering::Relaxed);
        self.suid.store(suid, Ordering::Relaxed);
        self.fsuid.store(euid, Ordering::Relaxed);
        if euid == 0 {
            self.caps.set_effective(self.caps.permitted());
        } else if old_euid == 0 {
            self.caps.set_effective(0);
            if uid != 0 && suid != 0 {
                self.caps.set_permitted(0);
            }
        }
    }
    pub fn set_gid_triplet(&self, gid: usize, egid: usize, sgid: usize) {
        self.gid.store(gid, Ordering::Relaxed);
        self.egid.store(egid, Ordering::Relaxed);
        self.sgid.store(sgid, Ordering::Relaxed);
        self.fsgid.store(egid, Ordering::Relaxed);
    }
    pub fn set_fsuid(&self, uid: usize) {
        self.fsuid.store(uid, Ordering::Relaxed);
    }
    pub fn set_fsgid(&self, gid: usize) {
        self.fsgid.store(gid, Ordering::Relaxed);
    }
    pub fn set_supplementary_groups(&self, groups: Vec<usize>) {
        *self.supplementary_groups.lock() = groups;
    }
    pub fn set_nice(&self, nice: i32) {
        self.sched.set_nice(nice);
    }
    pub fn set_sched(&self, policy: usize, priority: i32) {
        self.sched
            .set_sched(policy, priority, self.sched.reset_on_fork());
    }
    pub fn set_sched_with_reset_on_fork(&self, policy: usize, priority: i32, reset_on_fork: bool) {
        self.sched.set_sched(policy, priority, reset_on_fork);
    }
    pub fn set_cpu_affinity_mask(&self, mask: usize) {
        self.sched.set_cpu_affinity_mask(mask);
    }
    pub fn set_capabilities(&self, effective: usize, permitted: usize, inheritable: usize) {
        self.caps.set(effective, permitted, inheritable);
    }
    pub fn set_umask(&self, mask: usize) -> usize {
        self.umask.swap(mask & 0o777, Ordering::Relaxed)
    }
    pub fn set_cwd(&self, path: Arc<Path>) {
        *self.cwd.lock() = path;
    }
    pub fn set_root(&self, path: Arc<Path>) {
        *self.root.lock() = path;
    }
    pub fn set_exe_path(&self, path: String) {
        *self.exe_path.lock() = path;
    }
    pub fn set_exit_code(&self, exit_code: i32) {
        self.process.set_wait_status((exit_code & 0xff) << 8);
    }
    pub fn set_exit_signal(&self, signal: i32) {
        let signal = signal & 0x7f;
        let core_dumped = ActionType::default(Sig::from(signal)) == ActionType::Core;
        self.process
            .set_wait_status(signal | if core_dumped { 0x80 } else { 0 });
    }
    pub fn set_wait_event(&self, code: i32, status: i32) {
        self.process.set_wait_event(code, status);
    }
    pub fn take_wait_event(&self) -> Option<(i32, i32)> {
        self.process.take_wait_event()
    }
    pub fn peek_wait_event(&self) -> Option<(i32, i32)> {
        self.process.peek_wait_event()
    }
    pub fn notify_parent_sigchld(&self, code: i32) {
        if let Some(parent_process) = self.process.parent() {
            if let Some(parent) = parent_process.signal_target() {
                let action = parent.op_sig_handler(|handler| handler.get(Sig::SIGCHLD));
                if !action.flags.contains(SigActionFlag::SA_NOCLDSTOP) {
                    let siginfo =
                        SigInfo::new(Sig::SIGCHLD.raw(), code, SiField::Kill { tid: self.tgid() });
                    parent.receive_siginfo(siginfo, false);
                }
                wake_process_child_waiters(&parent_process);
            }
        }
    }
    pub fn notify_parent_exit(&self, code: i32) {
        notify_process_parent_exit(&self.process, code);
    }
    pub fn set_waiting_for_child(&self, waiting: bool) {
        self.waiting_for_child.store(waiting, Ordering::Release);
    }
    pub fn is_waiting_for_child(&self) -> bool {
        self.waiting_for_child.load(Ordering::Acquire)
    }
    pub fn exited_child_ids(&self) -> Vec<usize> {
        self.process.exited_child_ids()
    }
    pub fn remove_exited_child(&self, tid: usize) {
        self.process.remove_exited_child(tid);
    }
    pub fn clear_exited_children(&self) {
        self.process.clear_exited_children();
    }
    pub fn set_vfork_parent(&self, parent: &Arc<TaskControlBlock>) {
        *self.vfork_parent.lock() = Some(Arc::downgrade(parent));
    }
    /// 在 exec 成功后或 child 退出时，恰好唤醒一次 CLONE_VFORK parent。
    pub fn release_vfork_parent(&self) {
        let parent = self.vfork_parent.lock().take();
        if let Some(parent) = parent.and_then(|parent| parent.upgrade()) {
            crate::task::scheduler::wakeup_task(parent.tid());
        }
    }
    // 添加子任务
    pub fn add_child(&self, task: Arc<TaskControlBlock>) {
        let child_process = task.process();
        self.process.add_child(child_process);
    }
    // 任务状态设置
    pub fn set_ready(&self) {
        *self.task_status.lock() = TaskStatus::Ready;
    }
    pub fn set_running(&self) {
        *self.task_status.lock() = TaskStatus::Running;
    }
    pub fn set_blocked(&self) {
        *self.task_status.lock() = TaskStatus::Blocked;
    }
    pub fn set_stopped(&self) {
        *self.task_status.lock() = TaskStatus::Stopped;
    }
    pub fn set_exited(&self) {
        *self.task_status.lock() = TaskStatus::Exited;
    }

    // tid_address 设置
    pub fn set_clear_child_tid(&self, addr: usize) {
        self.inner.lock().tid_address.clear_child_tid = (addr != 0).then_some(addr);
    }

    pub fn set_set_child_tid(&self, addr: usize) {
        self.inner.lock().tid_address.set_child_tid = Some(addr);
    }

    pub fn clear_child_tid_addr(&self) -> Option<usize> {
        self.inner.lock().tid_address.clear_child_tid
    }
    pub fn set_robust_list(&self, head: usize, len: usize) {
        let mut inner = self.inner.lock();
        inner.tid_address.robust_list_head = (head != 0).then_some(head);
        inner.tid_address.robust_list_len = len;
    }
    pub fn robust_list(&self) -> Option<(usize, usize)> {
        let inner = self.inner.lock();
        inner
            .tid_address
            .robust_list_head
            .map(|head| (head, inner.tid_address.robust_list_len))
    }

    fn reset_tid_address_for_exec(&self) {
        self.inner.lock().tid_address = TidAddress::new();
    }

    // exec 时关闭线程组中除自身外的其它线程，只清理线程私有状态。
    pub fn close_other_threads_for_exec(&self) {
        let self_tid = self.tid();

        let tasks = self.op_thread_group(|tg| {
            tg.iter()
                .filter(|task| task.tid() != self_tid)
                .collect::<Vec<_>>()
        });
        if !tasks.is_empty() {
            debug_trace!(
                "[quiescetrace] exec tid={} siblings={:?}",
                self_tid,
                tasks.iter().map(|task| task.tid()).collect::<Vec<_>>()
            );
        }

        // 从 scheduler/task map 删除任务不会停止已在另一 CPU 执行的 sibling。先把所有 sibling
        // 标为不可运行，再等待 owner CPU 在 `__switch` 后确认，之后 exec 才能回收旧 image frame。
        for task in &tasks {
            task.request_termination();
            remove_task(task.tid());
        }

        while tasks.iter().any(|task| task.has_cpu_owner()) {
            crate::perf::quiescence_yield(1);
            crate::task::yield_current_task();
        }
        if !tasks.is_empty() {
            debug_trace!("[quiescetrace] exec remote-ack tid={}", self_tid);
        }

        for task in tasks {
            cleanup_exiting_thread(&task);
            task.set_exited();
            task.op_thread_group_mut(|tg| tg.remove(&task.tid()));
            TASK_MANAGER.remove(task.tid());
        }
    }

    /// Linux de-thread：所有 sibling quiescent 后，exec caller 接管稳定的进程 TGID 作为可见
    /// TID。内核栈分配与数字 TID 无关，因此这里只重新建立 identity index。
    fn adopt_process_tgid_for_exec(self: &Arc<Self>) {
        let old_tid = self.tid();
        let tgid = self.tgid();
        if old_tid == tgid {
            return;
        }

        TASK_MANAGER.remove(old_tid);
        self.process.remove_member(old_tid);
        self.op_thread_group_mut(|group| group.remove(&old_tid));

        *self.tid.write() = TidHandle(tgid);

        self.op_thread_group_mut(|group| group.add(self.clone()));
        self.process.add_member(self);
        TASK_MANAGER.add(self);
    }

    /* ======= 操作内部数据 ====== */
    pub(crate) fn memory_set_arc(&self) -> Arc<RwLock<MemorySet>> {
        self.memory_set.lock().clone()
    }

    pub fn op_memory_set_read<T>(&self, f: impl FnOnce(&MemorySet) -> T) -> T {
        let memory_set = self.memory_set_arc();
        f(&memory_set_read(&memory_set))
    }
    pub fn op_memory_set_write<T>(&self, f: impl FnOnce(&mut MemorySet) -> T) -> T {
        let memory_set = self.memory_set_arc();
        f(&mut memory_set_write(&memory_set))
    }
    pub fn op_process_children_mut<T>(
        &self,
        f: impl FnOnce(&mut BTreeMap<usize, Arc<ProcessState>>) -> T,
    ) -> T {
        self.process.op_children_mut(f)
    }
    // 只读查信号队列
    pub fn op_sig_pending<T>(&self, f: impl FnOnce(&SigPending) -> T) -> T {
        f(&self.sig_pending.lock())
    }

    // 可写改信号队列（加信号、改掩码）
    pub fn op_sig_pending_mut<T>(&self, f: impl FnOnce(&mut SigPending) -> T) -> T {
        f(&mut self.sig_pending.lock())
    }

    /// 从线程队列或进程队列中选出可投递给本线程的最小 signal；进程队列使用本线程 mask 判断。
    pub fn find_pending_signal(&self) -> Option<Sig> {
        let (mask, thread_signal) =
            self.op_sig_pending(|pending| (pending.mask, pending.find_signal()));
        let process_signal = self
            .process
            .op_sig_pending(|pending| pending.find_signal_with_mask(mask));
        match (thread_signal, process_signal) {
            (Some(thread), Some(process)) if process.raw() < thread.raw() => Some(process),
            (Some(thread), _) => Some(thread),
            (None, process) => process,
        }
    }

    pub fn fetch_pending_signal(&self) -> Option<(Sig, SigInfo)> {
        let fetched = self.process.op_sig_pending_mut(|process_pending| {
            self.op_sig_pending_mut(|thread_pending| {
                let thread_signal = thread_pending.find_signal();
                let process_signal = process_pending.find_signal_with_mask(thread_pending.mask);
                let take_process = match (thread_signal, process_signal) {
                    (None, Some(_)) => true,
                    (Some(thread), Some(process)) => process.raw() < thread.raw(),
                    _ => false,
                };
                if take_process {
                    let sig = process_signal.unwrap();
                    let mut set = SigSet::empty();
                    set.add_signal(sig);
                    process_pending.fetch_signal_from_set(set)
                } else {
                    thread_pending.fetch_signal()
                }
            })
        });
        if fetched
            .as_ref()
            .is_some_and(|(sig, _)| sig.raw() > Sig::SIGLEGACYMAX.raw())
        {
            self.process.release_rt_signals(1);
        }
        fetched
    }

    pub fn pending_blocked_set(&self) -> SigSet {
        let (thread_pending, mask) = self.op_sig_pending(|pending| (pending.pending, pending.mask));
        let process_pending = self.process.op_sig_pending(|pending| pending.pending);
        (thread_pending | process_pending) & mask
    }

    pub fn has_pending_in_set(&self, set: SigSet) -> bool {
        let thread = self.op_sig_pending(|pending| !(pending.pending & set).is_empty());
        thread
            || self
                .process
                .op_sig_pending(|pending| !(pending.pending & set).is_empty())
    }

    /// 查看 sigtimedwait 候选但不消费，使用户 copy 失败时两个队列都保持不变；布尔值标识
    /// 候选是否来自进程作用域。
    pub fn peek_pending_in_set(&self, set: SigSet) -> Option<(Sig, SigInfo, bool)> {
        let thread = self.op_sig_pending(|pending| {
            pending
                .find_signal_in_set(set)
                .and_then(|sig| pending.get_info(sig).copied().map(|info| (sig, info)))
        });
        let process = self.process.op_sig_pending(|pending| {
            pending
                .find_signal_in_set(set)
                .and_then(|sig| pending.get_info(sig).copied().map(|info| (sig, info)))
        });
        match (thread, process) {
            (Some((thread_sig, _thread_info)), Some((process_sig, process_info)))
                if process_sig.raw() < thread_sig.raw() =>
            {
                Some((process_sig, process_info, true))
            }
            (Some((sig, info)), _) => Some((sig, info, false)),
            (None, Some((sig, info))) => Some((sig, info, true)),
            (None, None) => None,
        }
    }

    pub fn consume_pending_signal(&self, sig: Sig, expected: SigInfo, process_scope: bool) -> bool {
        let mut set = SigSet::empty();
        set.add_signal(sig);
        let consumed = if process_scope {
            self.process.op_sig_pending_mut(|pending| {
                if pending.get_info(sig).copied() != Some(expected) {
                    return false;
                }
                pending.fetch_signal_from_set(set).is_some()
            })
        } else {
            self.op_sig_pending_mut(|pending| {
                if pending.get_info(sig).copied() != Some(expected) {
                    return false;
                }
                pending.fetch_signal_from_set(set).is_some()
            })
        };
        if consumed && sig.raw() > Sig::SIGLEGACYMAX.raw() {
            self.process.release_rt_signals(1);
        }
        consumed
    }

    // 只读查 handler 表
    pub fn op_sig_handler<T>(&self, f: impl FnOnce(&SigHandler) -> T) -> T {
        f(&self.sig_handler.lock())
    }

    // 可写改 handler 表（sigaction）
    pub fn op_sig_handler_mut<T>(&self, f: impl FnOnce(&mut SigHandler) -> T) -> T {
        f(&mut self.sig_handler.lock())
    }

    // 取信号栈
    pub fn sigstack(&self) -> Option<SignalStack> {
        let stack = *self.sig_stack.lock();
        if stack.ss_flags == (SS_DISABLE as i32) || stack.ss_size == 0 {
            None
        } else {
            Some(stack)
        }
    }

    pub fn raw_sigstack(&self) -> SignalStack {
        *self.sig_stack.lock()
    }

    pub fn set_sigstack(&self, stack: SignalStack) {
        *self.sig_stack.lock() = stack;
    }

    pub fn set_sigsuspend_saved_mask(&self, mask: Option<SigSet>) {
        self.inner.lock().sigsuspend_saved_mask = mask;
    }

    /// 取出并清除 sigsuspend 保存的原始信号掩码。
    ///
    /// 该值只能被实际获选投递的一个信号帧消费；普通 wake、伪唤醒或未处理 pending 信号
    /// 不得提前清除，否则 sigreturn 无法恢复进入 sigsuspend 前的线程状态。
    pub fn take_sigsuspend_saved_mask(&self) -> Option<SigSet> {
        self.inner.lock().sigsuspend_saved_mask.take()
    }

    // 信号入口：给线程发送信号
    // pub fn receive_siginfo(&self, siginfo: SigInfo, thread_level: bool) {
    //     let sig = crate::signal::Sig::from(siginfo.signo);
    //     match thread_level {
    //         true => {
    //             self.op_sig_pending_mut(|pending| pending.add_signal(siginfo));
    //             // SIGKILL/SIGSTOP 必须立即唤醒阻塞的线程，否则信号永远不会被处理
    //             if sig.is_kill_or_stop() && self.is_blocked() {
    //                 crate::task::scheduler::wakeup_task(self.tid());
    //             }
    //         }
    //         false => {
    //             let target = self.op_thread_group(|tg| {
    //                 let mut fallback = None;
    //                 let mut leader = None;
    //                 for task in tg.iter() {
    //                     if fallback.is_none() {
    //                         fallback = Some(task.clone());
    //                     }
    //                     if task.is_process_leader() {
    //                         leader = Some(task.clone());
    //                     }
    //                     let can_take_now = task.op_sig_pending(|pending| {
    //                         !pending.mask.contain_signal(sig) || sig.is_kill_or_stop()
    //                     });
    //                     if can_take_now {
    //                         return Some(task.clone());
    //                     }
    //                 }
    //                 leader.or(fallback)
    //             });
    //             if let Some(task) = target {
    //                 task.op_sig_pending_mut(|pending| pending.add_signal(siginfo));
    //                 // SIGKILL/SIGSTOP 必须立即唤醒阻塞的线程
    //                 if sig.is_kill_or_stop() && task.is_blocked() {
    //                     crate::task::scheduler::wakeup_task(task.tid());
    //                 }
    //             }
    //         }
    //     }
    // }
    fn is_disabled_musl_sigcancel(&self, sig: Sig) -> bool {
        if sig.raw() != 33 {
            return false;
        }

        #[cfg(target_arch = "riscv64")]
        let tp = self.get_trap_cx().x[4];
        #[cfg(target_arch = "loongarch64")]
        let tp = self.get_trap_cx().x[2];
        if tp < 152 {
            return false;
        }

        let mut cancel_state = 0u8;
        copy_from_user(&mut cancel_state as *mut u8, (tp - 152) as *const u8, 1).is_ok()
            && cancel_state == 1
    }

    pub fn receive_siginfo(&self, siginfo: SigInfo, thread_level: bool) {
        let _ = self.try_receive_siginfo(siginfo, thread_level, usize::MAX);
    }

    /// 将信号可靠地加入线程级或进程级 pending 队列，并唤醒合适的可中断等待者。
    ///
    /// 标准信号遵循合并语义；实时信号先原子预留调用者指定的 `RLIMIT_SIGPENDING` 额度，
    /// 只有额度不足时返回 `false`。进程定向信号保存在稳定的 `ProcessState` 中，选择某个
    /// 线程仅用于投递/唤醒提示，不能让信号生命周期依赖线程组首领 TCB。
    ///
    /// 唤醒前会优先匹配 `sigtimedwait`，再处理普通可中断系统调用和 futex 等待。
    /// `mark_signal_interrupted` 负责关闭“信号已被另一 CPU 消费、迟到提示污染下一次系统调用”
    /// 的竞争窗口；SIGKILL/SIGSTOP 路径保留强制唤醒兜底。
    pub fn try_receive_siginfo(
        &self,
        siginfo: SigInfo,
        thread_level: bool,
        rt_limit: usize,
    ) -> bool {
        let sig = crate::signal::Sig::from(siginfo.signo);
        let realtime = sig.raw() > Sig::SIGLEGACYMAX.raw();
        if realtime && !self.process.reserve_rt_signal(rt_limit) {
            return false;
        }

        match thread_level {
            // ===== 线程级信号 =====
            true => {
                let queued = self.op_sig_pending_mut(|pending| pending.add_signal(siginfo));
                debug_assert!(queued || !realtime);

                if self.is_sigtimedwait_waiter_for(sig) && self.is_blocked() {
                    crate::task::scheduler::wakeup_task(self.tid());
                }
                // ★ 改动：不只是 KILL/STOP 才唤醒，而是调用 check_signal_interrupt
                else if self.check_signal_interrupt() && !self.is_disabled_musl_sigcancel(sig) {
                    if self.mark_signal_interrupted() {
                        crate::task::futex::interrupt_futex_wait(self.tid());
                        if self.is_blocked() {
                            crate::task::scheduler::wakeup_task(self.tid());
                        }
                    }
                }
                // 保留原来的 KILL/STOP 立即唤醒逻辑作为兜底
                else if sig.is_kill_or_stop() && self.is_blocked() {
                    self.interrupted.store(true, Ordering::Relaxed);
                    crate::task::futex::interrupt_futex_wait(self.tid());
                    crate::task::scheduler::wakeup_task(self.tid());
                }
            }

            // ===== 进程级信号 =====
            false => {
                // process-directed signal 保留在进程所有的 storage 中，直到具体成员真正消费。
                // 被选成员可能在投递前 exit/exec，但不能因此丢失 signal。
                let queued = self.process.add_pending_signal(siginfo);
                debug_assert!(queued || !realtime);
                // sigtimedwait 等待的 process-directed signal 必须送给 waiter，即使用户态通常
                // 已经 mask 了这个 signal。
                let target = self.op_thread_group(|tg| {
                    if let Some(waiter) =
                        tg.iter().find(|task| task.is_sigtimedwait_waiter_for(sig))
                    {
                        return Some(waiter);
                    }
                    let mut fallback = None;
                    let mut leader = None;
                    for task in tg.iter() {
                        if fallback.is_none() {
                            fallback = Some(task.clone());
                        }
                        if task.is_process_leader() {
                            leader = Some(task.clone());
                        }
                        let can_take_now = task.op_sig_pending(|pending| {
                            !pending.mask.contain_signal(sig) || sig.is_kill_or_stop()
                        });
                        if can_take_now {
                            return Some(task.clone());
                        }
                    }
                    leader.or(fallback)
                });

                if let Some(task) = target {
                    if task.is_sigtimedwait_waiter_for(sig) && task.is_blocked() {
                        crate::task::scheduler::wakeup_task(task.tid());
                    }
                    // ★ 改动：同样使用 check_signal_interrupt
                    else if task.check_signal_interrupt() && !task.is_disabled_musl_sigcancel(sig)
                    {
                        if task.mark_signal_interrupted() {
                            crate::task::futex::interrupt_futex_wait(task.tid());
                            if task.is_blocked() {
                                crate::task::scheduler::wakeup_task(task.tid());
                            }
                        }
                    } else if sig.is_kill_or_stop() && task.is_blocked() {
                        task.interrupted.store(true, Ordering::Relaxed);
                        crate::task::futex::interrupt_futex_wait(task.tid());
                        crate::task::scheduler::wakeup_task(task.tid());
                    }
                }
            }
        }
        true
    }
    pub fn set_sig_context_addr(&self, addr: usize) {
        self.inner.lock().sig_context_addr = addr;
    }

    pub fn sig_context_addr(&self) -> usize {
        self.inner.lock().sig_context_addr
    }

    pub fn real_timer_remaining_ms(&self) -> usize {
        self.itimer_remaining_ms(0)
    }

    pub fn real_timer_interval_ms(&self) -> usize {
        self.itimer_interval_ms(0)
    }

    fn itimer_fields(&self, which: usize) -> Option<(&AtomicUsize, &AtomicUsize, Sig)> {
        self.itimers.fields(which)
    }

    pub fn itimer_remaining_ms(&self, which: usize) -> usize {
        let Some((deadline, _, _)) = self.itimer_fields(which) else {
            return 0;
        };
        let deadline = deadline.load(Ordering::Relaxed);
        if deadline == 0 {
            return 0;
        }
        deadline.saturating_sub(get_timeout_ms())
    }

    pub fn itimer_interval_ms(&self, which: usize) -> usize {
        let Some((_, interval, _)) = self.itimer_fields(which) else {
            return 0;
        };
        interval.load(Ordering::Relaxed)
    }

    pub fn set_real_timer_ms(&self, value_ms: usize, interval_ms: usize) -> usize {
        self.set_itimer_ms(0, value_ms, interval_ms)
    }

    pub fn set_itimer_ms(&self, which: usize, value_ms: usize, interval_ms: usize) -> usize {
        let Some((deadline_ref, interval_ref, _)) = self.itimer_fields(which) else {
            return 0;
        };
        let old_remaining = self.itimer_remaining_ms(which);
        let deadline = if value_ms == 0 {
            0
        } else {
            get_timeout_ms().saturating_add(value_ms)
        };
        deadline_ref.store(deadline, Ordering::Relaxed);
        interval_ref.store(interval_ms, Ordering::Relaxed);
        if deadline != 0 {
            ACTIVE_ITIMER_TASKS.lock().insert(self.tgid());
        } else if !self.has_active_itimer() {
            ACTIVE_ITIMER_TASKS.lock().remove(&self.tgid());
        }
        old_remaining
    }

    pub fn check_real_timer(&self) {
        self.check_itimer(0);
        self.check_itimer(1);
        self.check_itimer(2);
        if !self.has_active_itimer() {
            ACTIVE_ITIMER_TASKS.lock().remove(&self.tgid());
        }
    }

    fn has_active_itimer(&self) -> bool {
        (0..3).any(|which| {
            self.itimer_fields(which)
                .is_some_and(|(deadline, _, _)| deadline.load(Ordering::Relaxed) != 0)
        })
    }

    fn check_itimer(&self, which: usize) {
        let Some((deadline_ref, interval_ref, sig)) = self.itimer_fields(which) else {
            return;
        };
        let deadline = deadline_ref.load(Ordering::Relaxed);
        if deadline == 0 || get_timeout_ms() < deadline {
            return;
        }

        let interval = interval_ref.load(Ordering::Relaxed);
        let next_deadline = if interval == 0 {
            0
        } else {
            get_timeout_ms().saturating_add(interval)
        };
        if deadline_ref
            .compare_exchange(
                deadline,
                next_deadline,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_err()
        {
            return;
        }

        let siginfo = SigInfo::new(sig.raw(), SigInfo::KERNEL, SiField::None);
        self.receive_siginfo(siginfo, false);
    }

    pub fn personality(&self) -> usize {
        self.personality.load(Ordering::Relaxed)
    }

    pub fn set_personality(&self, personality: usize) -> usize {
        self.personality.swap(personality, Ordering::Relaxed)
    }

    pub(crate) fn begin_cpu_run(&self, cpu: usize, now: usize) {
        let in_user = self.cpu_in_user.load(Ordering::Acquire);
        self.thread_cpu_clock.lock().begin_run(now, in_user);
        self.process_cpu_clock.lock().begin_run(cpu, now, in_user);
    }

    pub(crate) fn end_cpu_run(&self, cpu: usize, now: usize) {
        self.thread_cpu_clock.lock().end_run(now);
        self.process_cpu_clock.lock().end_run(cpu, now);
    }

    pub fn thread_cpu_clock(&self) -> CpuClockHandle {
        CpuClockHandle {
            source: CpuClockSource::Thread(self.thread_cpu_clock.clone()),
        }
    }

    pub fn process_cpu_clock(&self) -> CpuClockHandle {
        CpuClockHandle {
            source: CpuClockSource::Process(self.process_cpu_clock.clone()),
        }
    }

    pub fn thread_cpu_time_us(&self) -> usize {
        self.thread_cpu_clock().now_us()
    }

    pub fn process_cpu_time_us(&self) -> usize {
        self.process_cpu_clock().now_us()
    }

    pub fn enter_kernel_accounting(&self) {
        self.cpu_in_user.store(false, Ordering::Release);
        let now = get_time();
        self.thread_cpu_clock.lock().transition(now, false);
        self.process_cpu_clock
            .lock()
            .transition(crate::arch::smp::current_hart_id(), now, false);
    }

    pub fn leave_kernel_accounting(&self) {
        let now = get_time();
        self.thread_cpu_clock.lock().transition(now, true);
        self.process_cpu_clock
            .lock()
            .transition(crate::arch::smp::current_hart_id(), now, true);
        self.cpu_in_user.store(true, Ordering::Release);
    }

    pub fn thread_accounting_ticks(&self) -> (usize, usize) {
        let (user, system) = self.thread_cpu_clock.lock().accounting_ticks_at(get_time());
        (
            cpu_ticks_to_us(user).saturating_mul(CLK_TCK) / 1_000_000,
            cpu_ticks_to_us(system).saturating_mul(CLK_TCK) / 1_000_000,
        )
    }

    pub fn process_accounting_ticks(&self) -> (usize, usize) {
        let (user, system) = self
            .process_cpu_clock
            .lock()
            .accounting_ticks_at(get_time());
        (
            cpu_ticks_to_us(user).saturating_mul(CLK_TCK) / 1_000_000,
            cpu_ticks_to_us(system).saturating_mul(CLK_TCK) / 1_000_000,
        )
    }

    pub fn child_ticks(&self) -> (usize, usize) {
        self.process.child_ticks()
    }

    pub fn add_child_ticks(&self, utime: usize, stime: usize) {
        self.process.add_child_ticks(utime, stime);
    }

    pub fn thread_resource_usage(&self) -> super::ResourceUsageSnapshot {
        self.thread_resource_usage.snapshot()
    }

    pub fn process_resource_usage(&self) -> super::ResourceUsageSnapshot {
        self.process.resource_usage()
    }

    pub fn child_resource_usage(&self) -> super::ResourceUsageSnapshot {
        self.process.child_resource_usage()
    }

    pub fn add_child_resource_usage(&self, usage: super::ResourceUsageSnapshot) {
        self.process.add_child_resource_usage(usage);
    }

    pub fn note_maxrss_pages(&self, pages: usize) {
        self.process.note_maxrss_pages(pages);
    }

    pub fn note_minor_fault(&self) {
        self.thread_resource_usage.note_minor_fault();
        self.process.note_minor_fault();
    }

    pub fn note_major_fault(&self) {
        self.thread_resource_usage.note_major_fault();
        self.process.note_major_fault();
    }

    pub fn note_input_blocks(&self, blocks: usize) {
        self.thread_resource_usage.note_input_blocks(blocks);
        self.process.note_input_blocks(blocks);
    }

    pub fn note_output_blocks(&self, blocks: usize) {
        self.thread_resource_usage.note_output_blocks(blocks);
        self.process.note_output_blocks(blocks);
    }

    pub fn note_voluntary_context_switch(&self) {
        self.thread_resource_usage.note_voluntary_context_switch();
        self.process.note_voluntary_context_switch();
    }

    pub fn note_involuntary_context_switch(&self) {
        self.thread_resource_usage.note_involuntary_context_switch();
        self.process.note_involuntary_context_switch();
    }
    pub fn op_thread_group<T>(&self, f: impl FnOnce(&ThreadGroup) -> T) -> T {
        f(&self.thread_group.lock())
    }
    pub fn op_thread_group_mut<T>(&self, f: impl FnOnce(&mut ThreadGroup) -> T) -> T {
        f(&mut self.thread_group.lock())
    }

    // 文件描述符相关操作
    pub fn alloc_fd(&self, fd_entry: FdEntry) -> SysResult<usize> {
        self.fd_table.lock().alloc_fd(fd_entry)
    }
    pub fn alloc_fd_from(&self, fd_entry: FdEntry, min_fd: usize) -> SysResult<usize> {
        self.fd_table.lock().alloc_fd_from(fd_entry, min_fd)
    }
    pub fn set_fd(&self, fd: usize, fd_entry: FdEntry) -> SysResult<Option<FdEntry>> {
        self.fd_table.lock().set_fd(fd, fd_entry)
    }
    pub fn close(&self, fd: usize) -> SysResult {
        self.fd_table.lock().close(fd)
    }
    pub fn unshare_fd_table(&self) {
        let mut current = self.fd_table.lock();
        if Arc::strong_count(&current) > 1 {
            *current = FdTable::from_existed_user(&current);
        }
    }

    /// exec 应在应用 CLOEXEC 前解除 CLONE_FILES。被 `close_other_threads_for_exec` 删除的线程
    /// 可能通过遗留 kernel-stack Arc 保留旧共享表；若已无 live task 拥有它，应显式清空，
    /// 避免 stale TCB pin 住 pipe end。
    pub fn unshare_fd_table_for_exec(&self) {
        let old_table = {
            let mut current = self.fd_table.lock();
            if Arc::strong_count(&current) <= 1 {
                return;
            }
            let old = current.clone();
            *current = FdTable::from_existed_user(&old);
            old
        };
        let old_ptr = Arc::as_ptr(&old_table);
        let shared_by_live_task = TASK_MANAGER.snapshot().into_iter().any(|other| {
            if core::ptr::eq(other.as_ref(), self) {
                return false;
            }
            let other_table = other.fd_table.lock();
            Arc::as_ptr(&other_table) == old_ptr
        });
        if !shared_by_live_task {
            old_table.clear();
        }
    }
    pub fn get_fd_entry(&self, fd: usize) -> SysResult<FdEntry> {
        self.fd_table.lock().get_fd_entry(fd)
    }
    pub fn open_fds(&self) -> Vec<usize> {
        self.fd_table.lock().open_fds()
    }
    pub fn nofile_limit(&self) -> (usize, usize) {
        self.fd_table.lock().nofile_limit()
    }
    pub fn set_nofile_limit(&self, cur: usize, max: usize) -> SysResult {
        self.fd_table.lock().set_nofile_limit(cur, max)
    }
    pub fn fsize_limit(&self) -> (usize, usize) {
        self.limits.fsize_limit()
    }
    pub fn set_fsize_limit(&self, cur: usize, max: usize) -> SysResult {
        self.limits.set_fsize_limit(cur, max)
    }
    pub fn memlock_limit(&self) -> (usize, usize) {
        self.limits.rlimit(RLIMIT_MEMLOCK).unwrap()
    }
    pub fn set_memlock_limit(&self, cur: usize, max: usize) -> SysResult {
        self.limits.set_rlimit(RLIMIT_MEMLOCK, cur, max)
    }
    pub fn rlimit(&self, resource: usize) -> Option<(usize, usize)> {
        if resource == RLIMIT_NOFILE {
            Some(self.nofile_limit())
        } else {
            self.limits.rlimit(resource)
        }
    }
    pub fn validate_rlimit(&self, resource: usize, cur: usize, max: usize) -> SysResult {
        if resource >= RLIMIT_COUNT || cur > max {
            return Err(Errno::EINVAL);
        }
        if resource == RLIMIT_NOFILE && max > crate::config::FTB_RLIMIT {
            return Err(Errno::EINVAL);
        }
        Ok(())
    }
    pub fn set_rlimit(&self, resource: usize, cur: usize, max: usize) -> SysResult {
        self.validate_rlimit(resource, cur, max)?;
        if resource == RLIMIT_NOFILE {
            self.set_nofile_limit(cur, max)
        } else {
            self.limits.set_rlimit(resource, cur, max)
        }
    }
}

pub fn check_active_itimers() {
    let tids: Vec<usize> = ACTIVE_ITIMER_TASKS.lock().iter().copied().collect();
    for tid in tids {
        if let Some(task) = TASK_MANAGER.get(tid) {
            task.check_real_timer();
        } else {
            ACTIVE_ITIMER_TASKS.lock().remove(&tid);
        }
    }
}

impl TaskControlBlock {
    // 内核栈操作
    pub fn kstack(&self) -> usize {
        self.kernel_stack.get_top()
    }

    /// user trap 必须从固定的栈边界开始保存 TrapContext；调度器保存的
    /// `kstack()` 则可能已经指向一个可恢复的 TaskContext。
    pub fn kernel_stack_top_edge(&self) -> usize {
        self.kernel_stack.get_top_edge()
    }

    /// 任务被某个 CPU claim 后，在恢复其内核上下文前写入该 CPU 的
    /// `PerCpu` 指针。该字段只会写入未运行任务保存的 TaskContext。
    pub fn set_kernel_tp(&self, tp: usize) {
        let task_cx = self.kernel_stack.get_top() as *mut TaskContext;
        unsafe {
            (*task_cx).set_tp(tp);
        }
    }

    /// 覆盖当前未执行的 saved context 中的 root token。LA idle context 在恢复前使用它，
    /// 防止 scheduler 继承已回收的用户页表。
    #[cfg(target_arch = "loongarch64")]
    pub fn set_saved_mmu_token(&self, token: usize) {
        let task_cx = self.kernel_stack.get_top() as *mut TaskContext;
        unsafe {
            (*task_cx).set_mmu_token(token);
        }
    }

    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        let trap_cx_ptr = self.kernel_stack.get_top_edge() - core::mem::size_of::<TrapContext>();
        unsafe { &mut *(trap_cx_ptr as *mut TrapContext) }
    }
    // 克隆异常上下文，注意传入的栈指针应指向栈上异常上下文数据的位置
    fn clone_trap_cx(&self, kernel_stack_ptr: usize) {
        let src_trap_cx_ptr = (self.kernel_stack.get_top_edge()
            - core::mem::size_of::<TrapContext>())
            as *const TrapContext;
        let dst_trap_cx_ptr = kernel_stack_ptr as *mut TrapContext;
        unsafe {
            dst_trap_cx_ptr.write(src_trap_cx_ptr.read());
        }
    }
    // 修改任务上下文，注意传入的栈指针应指向栈上任务上下文数据的位置
    fn write_task_cx(self: &Arc<Self>, kernel_stack_ptr: usize) {
        let token = self.get_user_token();
        let task_cx_ptr = kernel_stack_ptr as *mut TaskContext;
        let mut task_cx = TaskContext::app_init_task_context(token);
        task_cx.set_tp(Arc::as_ptr(self) as usize);
        unsafe {
            task_cx_ptr.write(task_cx);
        }
    }
}

#[derive(Clone, Copy)]
enum ExitCause {
    Code(i32),
    Signal(i32),
}

impl ExitCause {
    fn apply_to(self, task: &TaskControlBlock) {
        match self {
            Self::Code(code) => task.set_exit_code(code),
            Self::Signal(signal) => task.set_exit_signal(signal),
        }
    }

    fn sigchld_code(self) -> i32 {
        match self {
            Self::Code(_) => SigInfo::CLD_EXITED,
            Self::Signal(_) => SigInfo::CLD_KILLED,
        }
    }
}

fn exit_thread_detached(task: Arc<TaskControlBlock>) {
    exit_thread_inner(task, false);
}

fn cleanup_exiting_thread(task: &Arc<TaskControlBlock>) {
    exit_robust_list(task);
    if let Some(ctid) = task.clear_child_tid_addr() {
        let zero: i32 = 0;
        let _ = copy_to_user(ctid as *mut i32, &zero as *const i32, 1);
        let _ = crate::task::futex::futex_wake_private(ctid, 1);
        let _ = crate::task::futex::futex_wake(ctid, 1, false);
    }
    crate::task::futex::remove_futex_waiter(task.tid());
    remove_task(task.tid());
    let discarded_rt = task.op_sig_pending_mut(|pending| {
        let count = pending.realtime_count();
        pending.clear_pending();
        count
    });
    task.process.release_rt_signals(discarded_rt);
    task.process.remove_member(task.tid());
}

fn exit_thread_inner(task: Arc<TaskControlBlock>, remove_from_thread_group: bool) {
    cleanup_exiting_thread(&task);
    if remove_from_thread_group {
        task.op_thread_group_mut(|tg| tg.remove(&task.tid()));
    }
    task.set_exited();
    TASK_MANAGER.remove(task.tid());
}

/// 在线程退出或 exec 丢弃旧映像前，按 Linux robust-list 协议修复其遗留 futex。
///
/// 从用户 head 读取链表首项、futex 偏移和 pending 项，最多遍历固定数量以防损坏链表
/// 拖死内核。对仍由退出 tid 持有的 futex 原子写入 OWNER_DIED、保留 WAITERS 位并唤醒
/// 一个等待者；用户地址不可读、溢出或链表成环时停止清理而不让退出失败。
///
/// 必须在旧 MemorySet 仍安装时调用，且不能持有 task/MM/全局 futex 队列锁跨用户拷贝。
fn exit_robust_list(task: &Arc<TaskControlBlock>) {
    const ROBUST_LIST_HEAD_SIZE: usize = core::mem::size_of::<usize>() * 3;
    const FUTEX_WAITERS: u32 = 0x8000_0000;
    const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
    const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
    const ROBUST_LIST_LIMIT: usize = 2048;

    let Some((head, len)) = task.robust_list() else {
        return;
    };
    if len != ROBUST_LIST_HEAD_SIZE {
        return;
    }

    let mut first_entry = 0usize;
    let mut futex_offset = 0isize;
    let mut pending = 0usize;
    if copy_from_user(&mut first_entry as *mut usize, head as *const usize, 1).is_err()
        || copy_from_user(
            &mut futex_offset as *mut isize,
            (head + core::mem::size_of::<usize>()) as *const isize,
            1,
        )
        .is_err()
        || copy_from_user(
            &mut pending as *mut usize,
            (head + core::mem::size_of::<usize>() * 2) as *const usize,
            1,
        )
        .is_err()
    {
        return;
    }

    let mut entry = first_entry;
    for _ in 0..ROBUST_LIST_LIMIT {
        if entry == 0 || entry == head {
            break;
        }
        handle_robust_entry(
            task.tid(),
            entry,
            futex_offset,
            FUTEX_WAITERS,
            FUTEX_OWNER_DIED,
            FUTEX_TID_MASK,
        );

        let mut next = 0usize;
        if copy_from_user(&mut next as *mut usize, entry as *const usize, 1).is_err() {
            break;
        }
        entry = next;
    }

    if pending != 0 {
        handle_robust_entry(
            task.tid(),
            pending,
            futex_offset,
            FUTEX_WAITERS,
            FUTEX_OWNER_DIED,
            FUTEX_TID_MASK,
        );
    }
}

fn handle_robust_entry(
    tid: usize,
    entry: usize,
    futex_offset: isize,
    futex_waiters: u32,
    futex_owner_died: u32,
    futex_tid_mask: u32,
) {
    let Some(futex_addr) = entry.checked_add_signed(futex_offset) else {
        return;
    };

    let mut value = 0u32;
    if copy_from_user(&mut value as *mut u32, futex_addr as *const u32, 1).is_err() {
        return;
    }
    if value & futex_tid_mask != tid as u32 {
        return;
    }

    let new_value = (value & futex_waiters) | futex_owner_died;
    let _ = copy_to_user(futex_addr as *mut u32, &new_value as *const u32, 1);
    let _ = crate::task::futex::futex_wake(futex_addr, 1, true);
    let _ = crate::task::futex::futex_wake(futex_addr, 1, false);
}

fn reparent_children_to_init(task: &Arc<TaskControlBlock>) {
    let children = task.op_process_children_mut(core::mem::take);
    task.clear_exited_children();
    for (_, child) in children {
        let exited = child.is_exited();
        let sigchld_code = if child.wait_status() & 0x7f != 0 {
            SigInfo::CLD_KILLED
        } else {
            SigInfo::CLD_EXITED
        };
        INITPROC.process.add_child(child.clone());
        if exited {
            notify_process_parent_exit(&child, sigchld_code);
        }
    }
}

fn notify_process_parent_exit(process: &Arc<ProcessState>, code: i32) {
    if let Some(parent_process) = process.parent() {
        if let Some(parent) = parent_process.signal_target() {
            let action = parent.op_sig_handler(|handler| handler.get(Sig::SIGCHLD));
            let explicitly_ignored = action.sa_handler == SIG_IGN;
            let auto_reap =
                explicitly_ignored || action.flags.contains(SigActionFlag::SA_NOCLDWAIT);

            if auto_reap {
                let removed = parent_process
                    .op_children_mut(|children| children.remove(&process.tgid()).is_some());
                parent_process.remove_exited_child(process.tgid());
                if removed {
                    process.mark_reaped();
                    PROCESS_MANAGER.remove(process.tgid());
                }
            } else {
                parent_process.mark_child_exited(process.tgid());
            }

            // 安装 handler 时，即使有 SA_NOCLDWAIT，Linux 仍发送 SIGCHLD；显式 SIG_IGN
            // 才会同时自动 reap 并抑制该 signal。
            if !explicitly_ignored {
                let siginfo = SigInfo::new(
                    Sig::SIGCHLD.raw(),
                    code,
                    SiField::Kill {
                        tid: process.tgid(),
                    },
                );
                parent.receive_siginfo(siginfo, false);
            }
            wake_process_child_waiters(&parent_process);
        }
    }
}

fn wake_process_child_waiters(parent_process: &Arc<ProcessState>) {
    // 读取 waiter flag 前先发布。若 wait 调用恰在事件前完成扫描但尚未注册，它会在注册后
    // 观察到 generation 变化并重新扫描，而不是永久睡眠。
    parent_process.publish_child_event();
    let waiters = parent_process
        .members()
        .into_iter()
        .filter(|task| task.is_waiting_for_child())
        .map(|task| task.tid())
        .collect::<Vec<_>>();
    debug_trace!(
        "[quiescetrace] parent={} waiters={:?}",
        parent_process.tgid(),
        waiters
    );
    for tid in waiters {
        crate::task::scheduler::wakeup_task(tid);
    }
}

/// 线程退出 - 对外接口
///
/// - 设置当前线程退出状态
/// - 处理 clear_child_tid
/// - 释放线程私有资源
/// - 从线程组中移除自己
/// - 最后一个线程则进入进程级退出流程（目前未实现）
/// TODO: 当前退出线程组主线程时会退出整个线程组，与 Linux 实现存在差异
pub fn task_exit(task: Arc<TaskControlBlock>, exit_code: i32) {
    // warn! {"[kernel] Thread exit. tid: {}, tgid: {}, thread_count: {}.", task.tid(), task.tgid(), task.op_thread_group(|tg| tg.iter().count())}

    let exits_as_thread = task.op_thread_group_mut(|group| {
        if task.group_exiting.load(Ordering::Acquire)
            || task.process.lifecycle() != ProcessLifecycle::Running
            || group.size() == 1
        {
            false
        } else {
            group.remove(&task.tid());
            true
        }
    });
    if exits_as_thread {
        exit_thread_inner(task, false);
    } else {
        exit_process_group(task, ExitCause::Code(exit_code));
    }
}

pub fn task_exit_by_signal(task: Arc<TaskControlBlock>, signal: i32) {
    exit_process_group(task, ExitCause::Signal(signal));
}

/// 进程退出
///
/// - 杀掉/停止线程组内所有线程
/// - 关闭文件描述符
/// - 释放地址空间
/// - 释放信号处理、文件系统上下文等共享资源
/// - 给父进程发送 SIGCHLD
/// - 留下 exited 状态，等待父进程 wait 回收
pub fn task_group_exit(task: Arc<TaskControlBlock>, exit_code: i32) {
    exit_process_group(task, ExitCause::Code(exit_code));
}

/// 由唯一获胜线程完成整个进程组的不可逆退出事务。
///
/// `begin_exit` 决定 teardown owner；失败的并发调用者只能等待所有者把自身提交为 Exited，
/// 不能从本应不返回的 exit 路径继续执行。所有者先请求远端 sibling 退出并等待其 CPU
/// 上下文完成交接，之后才允许释放线程组共享的地址空间、文件表和信号资源。
///
/// `CLONE_VM`/`CLONE_FILES` 可能跨进程共享资源，因此是否拥有最终清理权以其他存活 tgid
/// 的实际引用为准，而不能只看 Arc 计数。函数还负责孤儿进程组通知、子进程托孤、共享
/// 文件映射回写、SysV SHM 分离、SIGCHLD/wait 状态发布以及 zombie 生命周期提交。
fn exit_process_group(task: Arc<TaskControlBlock>, cause: ExitCause) {
    debug_trace!(
        "[quiescetrace] group-exit begin tid={} tgid={}",
        task.tid(),
        task.tgid()
    );
    let orphan_candidates = {
        let sid = task.sid();
        let mut groups = alloc::vec![task.pgid()];
        task.op_process_children_mut(|children| {
            for child in children.values() {
                if child.sid() == sid && !groups.contains(&child.pgid()) {
                    groups.push(child.pgid());
                }
            }
        });
        groups
            .into_iter()
            .map(|pgid| {
                (
                    sid,
                    pgid,
                    crate::fs::tty::process_group_is_orphaned(sid, pgid),
                )
            })
            .collect::<Vec<_>>()
    };
    if !task.process.begin_exit() {
        // 另一线程拥有 group teardown。若 caller 仍为 Running 就返回，exit_group 的 scheduler
        // 路径会再次把它作为 runnable handoff，最终从本应不返回的 exit 之后继续执行。持续
        // yield，直到 teardown owner 把此 TCB 提交为 Exited；idle loop 不会重新发布 exited handoff。
        while !task.is_exited()
            && !matches!(
                task.process.lifecycle(),
                ProcessLifecycle::Zombie | ProcessLifecycle::Reaped
            )
        {
            crate::perf::quiescence_yield(1);
            crate::task::yield_current_task();
        }
        return;
    }
    task.group_exiting.store(true, Ordering::Release);

    let tgid = task.tgid();
    let threads = task.op_thread_group(|tg| tg.iter().collect::<Vec<_>>());

    // group-exit owner 可能拆除仍被其他 CPU 上 sibling 共享的 MemorySet。先请求它们 retire，
    // 等每个 owner CPU 发布 switch 后确认，之后才能释放公共地址空间和 file-backed frame。
    for thread in threads.iter().filter(|thread| thread.tid() != task.tid()) {
        thread.request_termination();
        remove_task(thread.tid());
    }
    while threads
        .iter()
        .filter(|thread| thread.tid() != task.tid())
        .any(|thread| thread.has_cpu_owner())
    {
        crate::perf::quiescence_yield(1);
        crate::task::yield_current_task();
    }
    debug_trace!(
        "[quiescetrace] group-exit remote-ack tid={} threads={}",
        task.tid(),
        threads.len()
    );

    let leader = threads
        .iter()
        .find(|thread| thread.tid() == tgid)
        .cloned()
        .unwrap_or_else(|| task.clone());

    // 独立 CLONE_VM 进程可以共享此地址空间，但裸 Arc count 无法识别：同一 thread group 的
    // detached/deferred TCB 也会短暂保留 MemorySet handle。只有另一 tgid 中的 live task 才算
    // external owner；否则 worker 恰在 leader 前退出的进程会跳过回收，让 zombie leader pin
    // 住所有 resident page。
    let task_memory_set = task.memory_set_arc();
    let live_tasks = TASK_MANAGER.snapshot();
    let memory_set_shared_outside_group = live_tasks.iter().any(|other| {
        if other.tgid() == tgid {
            return false;
        }
        Arc::ptr_eq(&other.memory_set_arc(), &task_memory_set)
    });
    let memory_set_owned_by_group = !memory_set_shared_outside_group;

    let fd_table_ptr = {
        let fd_table = task.fd_table.lock();
        Arc::as_ptr(&fd_table)
    };
    // Deferred TCB 离开 `thread_group` 后仍可能保留此 FdTable，因此 Arc count 无法区分它们
    // 与 live CLONE_FILES 进程。只有另一 tgid 中的 live task 才使清空共享表变得不安全。
    let fd_table_shared_outside_group = live_tasks.iter().any(|other| {
        if other.tgid() == tgid {
            return false;
        }
        let other_fd_table = other.fd_table.lock();
        Arc::as_ptr(&other_fd_table) == fd_table_ptr
    });
    let fd_table_owned_by_group = !fd_table_shared_outside_group;

    crate::fs::tty::release_console_on_session_exit(&task.process);

    let mut leader_cleaned = false;
    for thread in threads {
        if thread.tid() == leader.tid() {
            cleanup_exiting_thread(&thread);
            leader_cleaned = true;
        } else {
            exit_thread_detached(thread);
        }
    }
    if !leader_cleaned {
        cleanup_exiting_thread(&leader);
    }

    leader.op_thread_group_mut(|tg| tg.clear());

    // 修改孩子进程的父亲——托孤。children 是进程级资源，只处理一次。
    reparent_children_to_init(&task);
    for (sid, pgid, was_orphaned) in orphan_candidates {
        crate::fs::tty::notify_orphaned_process_group_transition(sid, pgid, was_orphaned);
    }

    let released_shm_attach_ids = if memory_set_owned_by_group {
        task.op_memory_set_read(|mem| mem.shm_attach_ids())
    } else {
        Vec::new()
    };
    let final_resident_pages = task.op_memory_set_read(|mem| mem.resident_page_count());
    task.note_maxrss_pages(final_resident_pages);
    if memory_set_owned_by_group {
        let writebacks = task.op_memory_set_read(|mem| mem.prepare_file_writeback(None));
        match writebacks {
            Ok(writebacks) => {
                if let Err(err) = writeback_file_pages(writebacks, false) {
                    println!("[task-exit] shared file mmap writeback failed: {:?}", err);
                }
            }
            Err(err) => {
                println!("[task-exit] shared file mmap snapshot failed: {:?}", err);
            }
        }
        task.op_memory_set_write(|mem| {
            mem.recycle_data_pages();
            mem.retire_asid();
        });
    }
    crate::syscall::ipc::release_shm_attachments(released_shm_attach_ids.as_slice(), tgid as i32);
    if fd_table_owned_by_group {
        task.fd_table.lock().clear();
    }
    crate::syscall::release_posix_locks_for_process(tgid);
    crate::syscall::remove_posix_timers_for_owner(tgid);

    cause.apply_to(&leader);
    let (user_ticks, system_ticks) = leader.process_accounting_ticks();
    task.process
        .publish_zombie(leader.wait_status(), user_ticks, system_ticks);
    leader.set_exited();
    leader.release_vfork_parent();
    leader.notify_parent_exit(cause.sigchld_code());

    // 清空残留信号
    leader.op_sig_pending_mut(|p| p.clear());

    TASK_MANAGER.remove(leader.tid());
    debug_trace!(
        "[quiescetrace] group-exit done tid={} leader={}",
        task.tid(),
        leader.tid()
    );
}

// 将命令行参数和环境变量压入用户栈
fn init_user_stack(
    memory_set: &mut MemorySet,
    exe_path: &str,
    args_vec: &[String],
    envs_vec: &[String],
    mut auxv: Vec<AuxHeader>,
    user_sp: &mut usize,
) -> SysResult<(usize, usize, usize, usize)> {
    const STACK_ALIGN: usize = 16;

    #[inline(always)]
    fn align_down(addr: usize) -> usize {
        addr & !(STACK_ALIGN - 1)
    }

    fn push_bytes_to_stack(
        memory_set: &mut MemorySet,
        bytes: &[u8],
        stack_ptr: &mut usize,
    ) -> SysResult<usize> {
        *stack_ptr -= bytes.len();
        memory_set.write_bytes_to_mapped_range(*stack_ptr, bytes)?;
        Ok(*stack_ptr)
    }

    fn push_strings_to_stack(
        memory_set: &mut MemorySet,
        strings: &[String],
        stack_ptr: &mut usize,
    ) -> SysResult<Vec<usize>> {
        let mut addresses = Vec::with_capacity(strings.len());

        for string in strings {
            let addr = push_bytes_to_stack(memory_set, &[0], stack_ptr)?;
            *stack_ptr = addr - string.len();
            memory_set.write_bytes_to_mapped_range(*stack_ptr, string.as_bytes())?;
            addresses.push(*stack_ptr);
        }

        *stack_ptr = align_down(*stack_ptr);
        Ok(addresses)
    }

    fn push_usize_to_stack(
        memory_set: &mut MemorySet,
        value: usize,
        stack_ptr: &mut usize,
    ) -> SysResult {
        *stack_ptr -= core::mem::size_of::<usize>();
        memory_set.write_bytes_to_mapped_range(*stack_ptr, &value.to_ne_bytes())
    }

    fn push_pointers_to_stack(
        memory_set: &mut MemorySet,
        pointers: &[usize],
        stack_ptr: &mut usize,
    ) -> SysResult<usize> {
        push_usize_to_stack(memory_set, 0, stack_ptr)?;
        for &ptr in pointers.iter().rev() {
            push_usize_to_stack(memory_set, ptr, stack_ptr)?;
        }
        Ok(*stack_ptr)
    }

    /// 按目标 ABI 的 `(type, value)` 对顺序把 auxv 逆序压入向下增长的用户栈。
    ///
    /// 逆序遍历保证最终内存顺序与输入一致，调用者必须已在尾部包含 `AT_NULL`。
    /// 每次写入通过目标 `MemorySet` 的安全代访接口解决惰性栈页；返回首个 auxv 项地址，
    /// 供初始寄存器和 `/proc` 相关布局使用。
    fn push_auxv_to_stack(
        memory_set: &mut MemorySet,
        auxv: &[AuxHeader],
        stack_ptr: &mut usize,
    ) -> SysResult<usize> {
        for header in auxv.iter().rev() {
            push_usize_to_stack(memory_set, header.value, stack_ptr)?;
            push_usize_to_stack(memory_set, header.aux_type, stack_ptr)?;
        }
        Ok(*stack_ptr)
    }

    *user_sp = align_down(*user_sp);

    // 字符串内容可以位于指针数组之上，argv/envp 数组保存实际地址。
    let envp = push_strings_to_stack(memory_set, envs_vec, user_sp)?;
    let argv = push_strings_to_stack(memory_set, args_vec, user_sp)?;
    let execfn_addr = {
        let addr = push_bytes_to_stack(memory_set, &[0], user_sp)?;
        *user_sp = addr - exe_path.len();
        memory_set.write_bytes_to_mapped_range(*user_sp, exe_path.as_bytes())?;
        let execfn_addr = *user_sp;
        *user_sp = align_down(*user_sp);
        execfn_addr
    };

    // —— 压入 AT_PLATFORM 字符串 ——
    #[cfg(target_arch = "riscv64")]
    let platform: &str = "RISC-V64";
    #[cfg(target_arch = "loongarch64")]
    let platform: &str = "loongarch64";

    *user_sp -= platform.len() + 1;
    *user_sp -= *user_sp % core::mem::size_of::<usize>();
    memory_set.write_bytes_to_mapped_range(*user_sp, platform.as_bytes())?;
    memory_set.write_bytes_to_mapped_range(*user_sp + platform.len(), &[0])?;
    let platform_addr = *user_sp;

    // —— 压入 16 字节随机数 ——
    *user_sp -= 16;
    let random_addr = *user_sp;
    let mut random = [0u8; 16];
    let mut seed = random_addr ^ args_vec.len() ^ envs_vec.len().wrapping_shl(8);
    for (idx, byte) in random.iter_mut().enumerate() {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed ^= (idx + 1usize).wrapping_mul(0x9e37_79b9);
        *byte = seed as u8;
    }
    memory_set.write_bytes_to_mapped_range(random_addr, &random)?;

    // —— 追加动态 aux 条目 ——
    #[cfg(target_arch = "loongarch64")]
    auxv.push(AuxHeader {
        aux_type: AT_HWCAP,
        value: crate::arch::elf_hwcap(),
    });
    auxv.push(AuxHeader {
        aux_type: AT_PLATFORM,
        value: platform_addr,
    });
    auxv.push(AuxHeader {
        aux_type: AT_RANDOM,
        value: random_addr,
    });
    auxv.push(AuxHeader {
        aux_type: AT_EXECFN,
        value: execfn_addr,
    });
    auxv.push(AuxHeader {
        aux_type: AT_NULL,
        value: 0,
    });

    // 预留填充空间，使压入 argc/argv/envp/auxv 后的最终 sp 仍保持 16 字节对齐。
    let pointer_count = 1                          // argc
        + argv.len() + 1                           // argv[] + NULL
        + envp.len() + 1                           // envp[] + NULL
        + auxv.len() * 2; // auxv (type + value pairs)
    let pointer_bytes = pointer_count * core::mem::size_of::<usize>();
    let padding = (STACK_ALIGN - pointer_bytes % STACK_ALIGN) % STACK_ALIGN;
    *user_sp -= padding;

    let auxv_base = push_auxv_to_stack(memory_set, &auxv, user_sp)?;
    let envp_base = push_pointers_to_stack(memory_set, &envp, user_sp)?;
    let argv_base = push_pointers_to_stack(memory_set, &argv, user_sp)?;
    push_usize_to_stack(memory_set, args_vec.len(), user_sp)?;

    Ok((argv_base, envp_base, auxv_base, *user_sp))
}

/// 线程组结构
pub struct ThreadGroup {
    member: BTreeMap<usize, Weak<TaskControlBlock>>,
}

impl ThreadGroup {
    pub fn new() -> Self {
        Self {
            member: BTreeMap::new(),
        }
    }
    pub fn size(&self) -> usize {
        self.member.len()
    }

    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.member.insert(task.tid(), Arc::downgrade(&task));
    }
    pub fn remove(&mut self, tid: &usize) {
        self.member.remove(tid);
    }

    pub fn clear(&mut self) {
        self.member.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = Arc<TaskControlBlock>> + '_ {
        self.member.values().filter_map(|task| task.upgrade())
    }
}

/// 任务状态
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    Ready,   // 已就绪
    Running, // 正在运行
    Blocked, // 阻塞
    Stopped, // 已被停止信号暂停
    Exited,  // 已退出
}

bitflags! {
    /// clone/fork 系统调用使用的标志位。
    ///
    /// Linux 的 clone 参数低 8 位不是普通的共享标志，而是子任务退出时
    /// 发送给父任务的信号编号；真正的 `CLONE_*` 标志从 bit 8 开始。
    pub struct CloneFlags: u32 {
        /// 退出信号掩码，低 8 位用于保存子任务退出时发送的信号编号。
        const EXIT_SIGNAL_MASK = 0xff;
        /// 子任务退出时向父任务发送 SIGCHLD。数值为 17，即退出信号编号。
        const SIGCHLD = 17;

        /// 共享地址空间；父子任务看到同一组用户虚拟内存映射。
        const CLONE_VM = 1 << 8;
        /// 共享文件系统上下文，例如当前工作目录和根目录。
        const CLONE_FS = 1 << 9;
        /// 共享文件描述符表；一方打开、关闭或替换 fd 会影响另一方。
        const CLONE_FILES = 1 << 10;
        /// 共享信号处理函数表。Linux 要求同时设置 `CLONE_VM`。
        const CLONE_SIGHAND = 1 << 11;
        /// 在父任务指定地址写入子任务 pidfd。
        const CLONE_PIDFD = 1 << 12;
        /// 子任务继续处于被 ptrace 跟踪状态。
        const CLONE_PTRACE = 1 << 13;
        /// 父任务阻塞到子任务 exec 或 exit；通常配合 vfork 语义使用。
        const CLONE_VFORK = 1 << 14;
        /// 子任务的父任务设为调用者的父任务，而不是调用者本身。
        const CLONE_PARENT = 1 << 15;
        /// 创建同一线程组内的新线程。Linux 要求同时设置 `CLONE_SIGHAND` 和 `CLONE_VM`。
        const CLONE_THREAD = 1 << 16;
        /// 为子任务创建新的 mount namespace。
        const CLONE_NEWNS = 1 << 17;
        /// 共享 System V semaphore undo 状态。
        const CLONE_SYSVSEM = 1 << 18;
        /// 设置子任务 TLS 指针。
        const CLONE_SETTLS = 1 << 19;
        /// 在父任务指定地址写入子任务 tid。
        const CLONE_PARENT_SETTID = 1 << 20;
        /// 子任务退出时清零指定地址并唤醒 futex 等待者。
        const CLONE_CHILD_CLEARTID = 1 << 21;
        /// 历史遗留标志，现代 Linux 基本忽略。
        const CLONE_DETACHED = 1 << 22;
        /// 阻止跟踪器强制对子任务设置 `CLONE_PTRACE`。
        const CLONE_UNTRACED = 1 << 23;
        /// 在子任务指定地址写入自己的 tid。
        const CLONE_CHILD_SETTID = 1 << 24;
        /// 为子任务创建新的 cgroup namespace。
        const CLONE_NEWCGROUP = 1 << 25;
        /// 为子任务创建新的 UTS namespace。
        const CLONE_NEWUTS = 1 << 26;
        /// 为子任务创建新的 IPC namespace。
        const CLONE_NEWIPC = 1 << 27;
        /// 为子任务创建新的 user namespace。
        const CLONE_NEWUSER = 1 << 28;
        /// 为子任务创建新的 PID namespace。
        const CLONE_NEWPID = 1 << 29;
        /// 为子任务创建新的 network namespace。
        const CLONE_NEWNET = 1 << 30;
        /// 共享 I/O 上下文。
        const CLONE_IO = 1 << 31;
    }
}

impl CloneFlags {
    /// 返回 clone 参数低 8 位携带的退出信号编号。
    pub fn exit_signal(self) -> u32 {
        self.bits() & Self::EXIT_SIGNAL_MASK.bits()
    }

    /// 返回除退出信号之外的 `CLONE_*` 共享/命名空间标志。
    pub fn clone_flags(self) -> Self {
        self & !Self::EXIT_SIGNAL_MASK
    }

    /// `CLONE_VM` 在 child exit/exec 前共享当前地址空间。每个 task 自己持有
    /// 可替换的 MemorySet handle，因此 exec 只会给调用者安装新地址空间，
    /// 不会覆盖仍由 vfork/clone parent 使用的旧地址空间。
    pub fn share_user_vm(self) -> bool {
        self.contains(Self::CLONE_VM)
    }
}
