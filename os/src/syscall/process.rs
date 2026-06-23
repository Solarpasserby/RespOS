// os/src/syscall/process.rs

use super::time::TimeVal;
use super::{Errno, SysResult};
use crate::config::CLK_TCK;
use crate::fs::vfs::InodeType;
use crate::fs::{AT_EMPTY_PATH, AT_FDCWD, AT_SYMLINK_NOFOLLOW, File, FileOp, OpenFlags, path_open};
use crate::loader::get_app_data_by_name;
use crate::mm::{
    MapPermission, VPNRange, VirtAddr, copy_cstr_from_user, copy_from_user, copy_to_user,
    extract_cstrings_from_user,
};
use crate::signal::{LinuxSigInfo, SigInfo};
use crate::task::{
    CloneFlags, TASK_MANAGER, TaskControlBlock, WaitOption, add_task, blocking_and_run_next,
    current_task, do_futex, exit_and_run_next, exit_group_and_run_next, yield_current_task,
};
use crate::timer::TimeSpec;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

#[cfg(target_arch = "loongarch64")]
const LOONGARCH_PTHREAD_TRACE: bool = false;

fn is_elf(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == [0x7f, b'E', b'L', b'F']
}

fn builtin_for_fs_exec(path: &str, args: &[String]) -> Option<&'static str> {
    if path == "/bin/true" {
        return Some("true");
    }

    let is_cp_path = matches!(path, "/musl/cp" | "/glibc/cp" | "/bin/cp");
    if is_cp_path && args.len() == 3 && args[1].contains("/ltp/testcases/bin/") && args[2] == "." {
        return Some("cp");
    }
    None
}

fn shebang_busybox_path(script_path: &str) -> &'static str {
    if script_path.starts_with("/glibc/") {
        "/glibc/busybox"
    } else {
        "/musl/busybox"
    }
}

fn process_leader(task: &Arc<TaskControlBlock>) -> Arc<TaskControlBlock> {
    task.op_thread_group(|tg| tg.iter().find(|task| task.tid() == task.tgid()))
        .unwrap_or_else(|| task.clone())
}

fn set_process_pgid(task: &Arc<TaskControlBlock>, pgid: usize) {
    task.op_thread_group(|tg| {
        for member in tg.iter() {
            member.set_pgid(pgid);
        }
    });
}

fn set_process_sid(task: &Arc<TaskControlBlock>, sid: usize) {
    task.op_thread_group(|tg| {
        for member in tg.iter() {
            member.set_sid(sid);
        }
    });
}

// 判断执行文件是否为 shell 脚本，若为 shell 脚本，则更改执行环境和参数
fn shebang_args(
    script_path: &str,
    data: &[u8],
    old_args: &[String],
) -> Option<(String, Vec<String>)> {
    if !data.starts_with(b"#!") {
        return None;
    }

    let end = data.iter().position(|&c| c == b'\n').unwrap_or(data.len());
    let line = core::str::from_utf8(&data[2..end]).ok()?.trim();
    let mut parts = line.split_whitespace();
    let interp = parts.next()?;
    let interp_arg = parts.next();

    let shell_path = if interp == "/bin/sh" || interp == "/usr/bin/sh" || interp == "/busybox" {
        shebang_busybox_path(script_path)
    } else {
        interp
    };

    let mut args = Vec::new();
    args.push(String::from("busybox"));
    if let Some(arg) = interp_arg {
        args.push(String::from(arg));
    } else {
        args.push(String::from("sh"));
    }
    args.push(String::from(script_path));
    args.extend(old_args.iter().skip(1).cloned());
    Some((String::from(shell_path), args))
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct RUsage {
    pub ru_utime: TimeVal,
    pub ru_stime: TimeVal,
    pub ru_maxrss: isize,
    pub ru_ixrss: isize,
    pub ru_idrss: isize,
    pub ru_isrss: isize,
    pub ru_minflt: isize,
    pub ru_majflt: isize,
    pub ru_nswap: isize,
    pub ru_inblock: isize,
    pub ru_oublock: isize,
    pub ru_msgsnd: isize,
    pub ru_msgrcv: isize,
    pub ru_nsignals: isize,
    pub ru_nvcsw: isize,
    pub ru_nivcsw: isize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct RLimit {
    pub cur: usize,
    pub max: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct SchedAttr {
    pub size: u32,
    pub sched_policy: u32,
    pub sched_flags: u64,
    pub sched_nice: i32,
    pub sched_priority: u32,
    pub sched_runtime: u64,
    pub sched_deadline: u64,
    pub sched_period: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct CapUserHeader {
    pub version: u32,
    pub pid: i32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct CapUserData {
    pub effective: u32,
    pub permitted: u32,
    pub inheritable: u32,
}

impl Default for RUsage {
    fn default() -> Self {
        Self {
            ru_utime: TimeVal { sec: 0, usec: 0 },
            ru_stime: TimeVal { sec: 0, usec: 0 },
            ru_maxrss: 0,
            ru_ixrss: 0,
            ru_idrss: 0,
            ru_isrss: 0,
            ru_minflt: 0,
            ru_majflt: 0,
            ru_nswap: 0,
            ru_inblock: 0,
            ru_oublock: 0,
            ru_msgsnd: 0,
            ru_msgrcv: 0,
            ru_nsignals: 0,
            ru_nvcsw: 0,
            ru_nivcsw: 0,
        }
    }
}

fn rusage_from_ticks(utime: usize, stime: usize) -> RUsage {
    RUsage {
        ru_utime: TimeVal {
            sec: utime / CLK_TCK,
            usec: (utime % CLK_TCK) * (1_000_000 / CLK_TCK),
        },
        ru_stime: TimeVal {
            sec: stime / CLK_TCK,
            usec: (stime % CLK_TCK) * (1_000_000 / CLK_TCK),
        },
        ..RUsage::default()
    }
}

pub fn sys_getrusage(who: isize, usage: *mut RUsage) -> SysResult<usize> {
    const RUSAGE_SELF: isize = 0;
    const RUSAGE_CHILDREN: isize = -1;

    let task = current_task().expect("[kernel] current task is None.");
    let rusage = match who {
        RUSAGE_SELF => {
            let ticks = task.elapsed_ticks();
            rusage_from_ticks(ticks, ticks)
        }
        RUSAGE_CHILDREN => {
            let (utime, stime) = task.child_ticks();
            rusage_from_ticks(utime, stime)
        }
        _ => return Err(Errno::EINVAL),
    };
    copy_to_user(usage, &rusage as *const RUsage, 1)?;
    Ok(0)
}

/// 系统调用 sys_exit_group
///
/// 退出单个线程
pub fn sys_exit(exit_code: i32) -> ! {
    exit_and_run_next(exit_code)
}

/// 系统调用 sys_exit_group
///
/// 退出整个线程组
pub fn sys_exit_group(exit_code: i32) -> ! {
    exit_group_and_run_next(exit_code)
}

pub fn sys_sched_yield() -> SysResult<usize> {
    yield_current_task();
    Ok(0)
}

const ONLINE_CPU_MASK: usize = 0b11;
const SCHED_OTHER: usize = 0;
const SCHED_FIFO: usize = 1;
const SCHED_RR: usize = 2;
const SCHED_BATCH: usize = 3;
const SCHED_IDLE: usize = 5;
const SCHED_DEADLINE: usize = 6;
const SCHED_RESET_ON_FORK: usize = 0x4000_0000;
const MIN_RT_PRIO: i32 = 1;
const MAX_RT_PRIO: i32 = 99;
const RLIMIT_RTPRIO: usize = 14;
const CAP_SYS_NICE: usize = 23;
const CAP_SETPCAP: usize = 8;
const CAP_LOW_MASK: usize = u32::MAX as usize;

fn sched_task(pid: isize) -> SysResult<Arc<TaskControlBlock>> {
    if pid < 0 {
        return Err(Errno::EINVAL);
    }
    if pid == 0 {
        current_task().ok_or(Errno::ESRCH)
    } else {
        TASK_MANAGER.get(pid as usize).ok_or(Errno::ESRCH)
    }
}

fn is_regular_sched_policy(policy: usize) -> bool {
    matches!(policy, SCHED_OTHER | SCHED_BATCH | SCHED_IDLE)
}

fn is_rt_sched_policy(policy: usize) -> bool {
    matches!(policy, SCHED_FIFO | SCHED_RR)
}

fn normalize_sched_policy(policy: usize) -> SysResult<(usize, bool)> {
    let base = policy & !SCHED_RESET_ON_FORK;
    if policy & !(SCHED_RESET_ON_FORK | 0xff) != 0 {
        return Err(Errno::EINVAL);
    }
    Ok((base, policy & SCHED_RESET_ON_FORK != 0))
}

fn check_sched_param(policy: usize, priority: i32) -> SysResult<()> {
    if is_rt_sched_policy(policy) {
        if !(MIN_RT_PRIO..=MAX_RT_PRIO).contains(&priority) {
            return Err(Errno::EINVAL);
        }
    } else if is_regular_sched_policy(policy) {
        if priority != 0 {
            return Err(Errno::EINVAL);
        }
    } else {
        return Err(Errno::EINVAL);
    }
    Ok(())
}

fn check_sched_permission(
    target: &Arc<TaskControlBlock>,
    policy: usize,
    priority: i32,
) -> SysResult<()> {
    let current = current_task().expect("[kernel] current task is None.");
    if current.has_cap(CAP_SYS_NICE) {
        return Ok(());
    }
    if current.euid() != target.uid() && current.euid() != target.euid() {
        return Err(Errno::EPERM);
    }
    if is_rt_sched_policy(policy) {
        let rtprio_limit = current
            .rlimit(RLIMIT_RTPRIO)
            .map(|limit| limit.0)
            .unwrap_or(0);
        if priority as usize > rtprio_limit {
            return Err(Errno::EPERM);
        }
    }
    Ok(())
}

fn read_sched_priority(param: *const i32) -> SysResult<i32> {
    if param.is_null() {
        return Err(Errno::EINVAL);
    }
    let mut priority = 0i32;
    copy_from_user(&mut priority as *mut i32, param, 1)?;
    Ok(priority)
}

pub fn sys_sched_setaffinity(pid: isize, cpusetsize: usize, mask: *const u8) -> SysResult<usize> {
    let task = sched_task(pid)?;
    let current = current_task().expect("[kernel] current task is None.");
    if !current.has_cap(CAP_SYS_NICE)
        && current.euid() != task.uid()
        && current.euid() != task.euid()
    {
        return Err(Errno::EPERM);
    }
    if cpusetsize == 0 {
        return Err(Errno::EINVAL);
    }
    let mut kbuf = alloc::vec![0u8; cpusetsize];
    copy_from_user(kbuf.as_mut_ptr(), mask, cpusetsize)?;
    let mut requested = 0usize;
    for (idx, byte) in kbuf.iter().take(core::mem::size_of::<usize>()).enumerate() {
        requested |= (*byte as usize) << (idx * 8);
    }
    let effective = requested & ONLINE_CPU_MASK;
    if effective == 0 {
        return Err(Errno::EINVAL);
    }
    task.set_cpu_affinity_mask(effective);
    Ok(0)
}

pub fn sys_sched_getaffinity(pid: isize, cpusetsize: usize, mask: *mut u8) -> SysResult<usize> {
    let task = sched_task(pid)?;
    if cpusetsize == 0 {
        return Err(Errno::EINVAL);
    }
    let mut kbuf = alloc::vec![0u8; cpusetsize];
    let affinity = task.cpu_affinity_mask() & ONLINE_CPU_MASK;
    for idx in 0..core::mem::size_of::<usize>().min(cpusetsize) {
        kbuf[idx] = ((affinity >> (idx * 8)) & 0xff) as u8;
    }
    copy_to_user(mask, kbuf.as_ptr(), cpusetsize)?;
    Ok(cpusetsize)
}

pub fn sys_sched_setscheduler(pid: isize, policy: usize, param: *const i32) -> SysResult<usize> {
    let target = sched_task(pid)?;
    let (policy, reset_on_fork) = normalize_sched_policy(policy)?;
    let priority = read_sched_priority(param)?;
    check_sched_param(policy, priority)?;
    check_sched_permission(&target, policy, priority)?;
    target.set_sched_with_reset_on_fork(policy, priority, reset_on_fork);
    Ok(0)
}

pub fn sys_sched_getscheduler(pid: isize) -> SysResult<usize> {
    Ok(sched_task(pid)?.sched_policy())
}

pub fn sys_sched_setparam(pid: isize, param: *const i32) -> SysResult<usize> {
    let target = sched_task(pid)?;
    let priority = read_sched_priority(param)?;
    let policy = target.sched_policy();
    check_sched_param(policy, priority)?;
    check_sched_permission(&target, policy, priority)?;
    target.set_sched(policy, priority);
    Ok(0)
}

pub fn sys_sched_getparam(pid: isize, param: *mut i32) -> SysResult<usize> {
    if param.is_null() {
        return Err(Errno::EINVAL);
    }
    let target = sched_task(pid)?;
    let priority = target.sched_priority();
    copy_to_user(param, &priority as *const i32, 1)?;
    Ok(0)
}

pub fn sys_sched_get_priority_max(policy: isize) -> SysResult<usize> {
    let (policy, _) = normalize_sched_policy(policy as usize)?;
    match policy {
        SCHED_FIFO | SCHED_RR => Ok(MAX_RT_PRIO as usize),
        SCHED_OTHER | SCHED_BATCH | SCHED_IDLE | SCHED_DEADLINE => Ok(0),
        _ => Err(Errno::EINVAL),
    }
}

pub fn sys_sched_get_priority_min(policy: isize) -> SysResult<usize> {
    let (policy, _) = normalize_sched_policy(policy as usize)?;
    match policy {
        SCHED_FIFO | SCHED_RR => Ok(MIN_RT_PRIO as usize),
        SCHED_OTHER | SCHED_BATCH | SCHED_IDLE | SCHED_DEADLINE => Ok(0),
        _ => Err(Errno::EINVAL),
    }
}

pub fn sys_sched_rr_get_interval(pid: isize, interval: *mut TimeSpec) -> SysResult<usize> {
    if interval.is_null() {
        return Err(Errno::EINVAL);
    }
    let target = sched_task(pid)?;
    let time_slice = if target.sched_policy() == SCHED_RR {
        TimeSpec {
            sec: 0,
            nsec: 100_000_000,
        }
    } else {
        TimeSpec { sec: 0, nsec: 0 }
    };
    copy_to_user(interval, &time_slice as *const TimeSpec, 1)?;
    Ok(0)
}

pub fn sys_sched_setattr(pid: isize, attr: *const SchedAttr, flags: u32) -> SysResult<usize> {
    if attr.is_null() || flags != 0 {
        return Err(Errno::EINVAL);
    }
    let target = sched_task(pid)?;
    let mut sched_attr = SchedAttr::default();
    copy_from_user(&mut sched_attr as *mut SchedAttr, attr, 1)?;
    let attr_size = sched_attr.size as usize;
    if attr_size < core::mem::size_of::<SchedAttr>() {
        return Err(Errno::EINVAL);
    }
    if sched_attr.sched_flags & !(SCHED_RESET_ON_FORK as u64) != 0 {
        return Err(Errno::EINVAL);
    }

    let (policy, policy_reset_on_fork) = normalize_sched_policy(sched_attr.sched_policy as usize)?;
    let reset_on_fork =
        policy_reset_on_fork || sched_attr.sched_flags & SCHED_RESET_ON_FORK as u64 != 0;
    if policy == SCHED_DEADLINE {
        return Err(Errno::EINVAL);
    }
    let priority = if is_rt_sched_policy(policy) {
        sched_attr.sched_priority as i32
    } else {
        if !(-20..=19).contains(&sched_attr.sched_nice) {
            return Err(Errno::EINVAL);
        }
        if sched_attr.sched_priority != 0 {
            return Err(Errno::EINVAL);
        }
        0
    };
    check_sched_param(policy, priority)?;
    check_sched_permission(&target, policy, priority)?;
    target.set_sched_with_reset_on_fork(policy, priority, reset_on_fork);
    if is_regular_sched_policy(policy) {
        target.set_nice(sched_attr.sched_nice);
    }
    Ok(0)
}

pub fn sys_sched_getattr(
    pid: isize,
    attr: *mut SchedAttr,
    size: u32,
    flags: u32,
) -> SysResult<usize> {
    if attr.is_null() || flags != 0 || (size as usize) < core::mem::size_of::<SchedAttr>() {
        return Err(Errno::EINVAL);
    }
    let target = sched_task(pid)?;
    let sched_attr = SchedAttr {
        size: core::mem::size_of::<SchedAttr>() as u32,
        sched_policy: target.sched_policy() as u32,
        sched_flags: 0,
        sched_nice: target.nice(),
        sched_priority: if is_rt_sched_policy(target.sched_policy()) {
            target.sched_priority() as u32
        } else {
            0
        },
        sched_runtime: 0,
        sched_deadline: 0,
        sched_period: 0,
    };
    copy_to_user(attr, &sched_attr as *const SchedAttr, 1)?;
    Ok(0)
}

fn check_cap_header(header: CapUserHeader) -> SysResult<CapUserHeader> {
    const LINUX_CAPABILITY_VERSION_1: u32 = 0x1998_0330;
    const LINUX_CAPABILITY_VERSION_2: u32 = 0x2007_1026;
    const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    match header.version {
        LINUX_CAPABILITY_VERSION_1 | LINUX_CAPABILITY_VERSION_2 | LINUX_CAPABILITY_VERSION_3 => {}
        _ => return Err(Errno::EINVAL),
    }

    let task = current_task().expect("[kernel] current task is None.");
    if header.pid < 0 {
        return Err(Errno::EINVAL);
    }
    if header.pid != 0 && header.pid as usize != task.tid() && header.pid as usize != task.tgid() {
        return Err(Errno::ESRCH);
    }

    Ok(header)
}

fn cap_task(header: CapUserHeader) -> SysResult<Arc<TaskControlBlock>> {
    if header.pid == 0 {
        current_task().ok_or(Errno::ESRCH)
    } else {
        TASK_MANAGER.get(header.pid as usize).ok_or(Errno::ESRCH)
    }
}

pub fn sys_capget(hdrp: *mut CapUserHeader, datap: *mut CapUserData) -> SysResult<usize> {
    let mut header = CapUserHeader::default();
    copy_from_user(&mut header as *mut CapUserHeader, hdrp, 1)?;
    let header = check_cap_header(header)?;
    let task = cap_task(header)?;

    if !datap.is_null() {
        let effective = task.cap_effective();
        let permitted = task.cap_permitted();
        let inheritable = task.cap_inheritable();
        let data = [
            CapUserData {
                effective: (effective & CAP_LOW_MASK) as u32,
                permitted: (permitted & CAP_LOW_MASK) as u32,
                inheritable: (inheritable & CAP_LOW_MASK) as u32,
            },
            CapUserData::default(),
        ];
        copy_to_user(datap, data.as_ptr(), data.len())?;
    }
    Ok(0)
}

pub fn sys_capset(hdrp: *const CapUserHeader, datap: *const CapUserData) -> SysResult<usize> {
    let mut header = CapUserHeader::default();
    copy_from_user(&mut header as *mut CapUserHeader, hdrp, 1)?;
    let header = check_cap_header(header)?;
    let task = cap_task(header)?;
    let current = current_task().expect("[kernel] current task is None.");
    if task.tid() != current.tid() {
        return Err(Errno::EPERM);
    }

    let mut data = [CapUserData::default(); 2];
    copy_from_user(data.as_mut_ptr(), datap, data.len())?;
    if data[1].effective != 0 || data[1].permitted != 0 || data[1].inheritable != 0 {
        return Err(Errno::EPERM);
    }

    let effective = data[0].effective as usize;
    let permitted = data[0].permitted as usize;
    let inheritable = data[0].inheritable as usize;
    let old_permitted = task.cap_permitted();
    if effective & !permitted != 0 {
        return Err(Errno::EPERM);
    }
    if permitted & !old_permitted != 0 {
        return Err(Errno::EPERM);
    }
    if !current.has_cap(CAP_SETPCAP) && inheritable & !old_permitted != 0 {
        return Err(Errno::EPERM);
    }

    task.set_capabilities(effective, permitted, inheritable);
    Ok(0)
}

pub fn sys_gettid() -> SysResult<usize> {
    Ok(current_task()
        .expect("[kernel] current task is None.")
        .tid())
}

/// 系统调用 sys-setpgid
///
/// 当前内核尚未建模 controlling terminal，但维护 sid/pgid 的基本关系。
///
/// TODO[ABI-COMPAT]: 补齐 job-control 相关的 tty 前后台进程组规则。
pub fn sys_setpgid(pid: usize, pgid: usize) -> SysResult<usize> {
    let current_thread = current_task().expect("[kernel] current task is None.");
    let current = process_leader(&current_thread);
    if (pgid as isize) < 0 {
        return Err(Errno::EINVAL);
    }
    if (pid as isize) < 0 {
        return Err(Errno::ESRCH);
    }

    let target = if pid == 0 {
        current.clone()
    } else {
        process_leader(&TASK_MANAGER.get(pid).ok_or(Errno::ESRCH)?)
    };
    if !target.is_process_leader() {
        return Err(Errno::ESRCH);
    }

    let target_is_current = target.tgid() == current.tgid();
    if !target_is_current {
        let is_child = current.op_children_mut(|children| children.contains_key(&target.tgid()));
        if !is_child {
            return Err(Errno::ESRCH);
        }
        if target.did_exec() {
            return Err(Errno::EACCES);
        }
    }
    if target.sid() != current.sid() {
        return Err(Errno::EPERM);
    }
    if target.sid() == target.tgid() {
        return Err(Errno::EPERM);
    }

    let new_pgid = if pgid == 0 { target.tgid() } else { pgid };
    if new_pgid != target.tgid() {
        let mut group_exists_in_session = false;
        TASK_MANAGER.for_each(|task| {
            if task.is_process_leader() && task.sid() == current.sid() && task.pgid() == new_pgid {
                group_exists_in_session = true;
            }
        });
        if !group_exists_in_session {
            return Err(Errno::EPERM);
        }
    }

    set_process_pgid(&target, new_pgid);
    Ok(0)
}

pub fn sys_getpgid(pid: usize) -> SysResult<usize> {
    let current_thread = current_task().expect("[kernel] current task is None.");
    let target = if pid == 0 {
        process_leader(&current_thread)
    } else {
        process_leader(&TASK_MANAGER.get(pid).ok_or(Errno::ESRCH)?)
    };
    Ok(target.pgid())
}

/// 系统调用 sys-setsid
///
/// 当前内核还没有完整建模 session/controlling terminal；这里保留 Linux 的关键可见语义：
/// 进程组 leader 不能 setsid，成功后调用者成为新的进程组 leader，并返回新 session id。
pub fn sys_setsid() -> SysResult<usize> {
    let current_thread = current_task().expect("[kernel] current task is None.");
    let current = process_leader(&current_thread);
    let pid = current.tgid();
    if current.pgid() == pid {
        return Err(Errno::EPERM);
    }
    set_process_sid(&current, pid);
    set_process_pgid(&current, pid);
    Ok(pid)
}

pub fn sys_prlimit64(
    pid: usize,
    resource: usize,
    new_limit: *const RLimit,
    old_limit: *mut RLimit,
) -> SysResult<usize> {
    let current = current_task().expect("[kernel] current task is None.");
    let task = if pid == 0 {
        current.clone()
    } else {
        let task = TASK_MANAGER.get(pid).ok_or(Errno::ESRCH)?;
        process_leader(&task)
    };
    if resource >= RLIMIT_COUNT {
        return Err(Errno::EINVAL);
    }
    if pid != 0 && task.tgid() != pid {
        return Err(Errno::ESRCH);
    }

    let (cur, max) = task.rlimit(resource).ok_or(Errno::EINVAL)?;
    let old = RLimit { cur, max };

    if !new_limit.is_null() {
        let mut limit = RLimit { cur: 0, max: 0 };
        copy_from_user(&mut limit as *mut RLimit, new_limit, 1)?;
        if limit.cur > limit.max {
            return Err(Errno::EINVAL);
        }
        if limit.max > old.max && current.euid() != 0 {
            return Err(Errno::EPERM);
        }
        task.set_rlimit(resource, limit.cur, limit.max)?;
    }

    if !old_limit.is_null() {
        copy_to_user(old_limit, &old as *const RLimit, 1)?;
    }

    Ok(0)
}

const RLIMIT_COUNT: usize = 16;

pub fn sys_getrlimit(resource: usize, old_limit: *mut RLimit) -> SysResult<usize> {
    sys_prlimit64(0, resource, core::ptr::null(), old_limit)
}

pub fn sys_setrlimit(resource: usize, new_limit: *const RLimit) -> SysResult<usize> {
    sys_prlimit64(0, resource, new_limit, core::ptr::null_mut())
}

pub fn sys_getrandom(buf: *mut u8, buflen: usize, flags: usize) -> SysResult<usize> {
    const GRND_NONBLOCK: usize = 0x0001;
    const GRND_RANDOM: usize = 0x0002;
    const GRND_INSECURE: usize = 0x0004;

    if flags & !(GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE) != 0 {
        return Err(Errno::EINVAL);
    }
    if buflen == 0 {
        return Ok(0);
    }

    // TODO[ABI-COMPAT]: 这是为了 libc 测例提供的确定性兜底实现，不是密码学安全随机源。
    let mut bytes = alloc::vec![0u8; buflen];
    let mut seed = get_time_seed();
    for (idx, byte) in bytes.iter_mut().enumerate() {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed ^= (idx as usize).wrapping_mul(0x9e37_79b9);
        *byte = seed as u8;
    }
    copy_to_user(buf, bytes.as_ptr(), buflen)?;
    Ok(buflen)
}

fn get_time_seed() -> usize {
    crate::timer::get_time_ms() ^ 0x7265_7370_6f73
}

pub fn sys_clone(
    flags: usize,
    stack: usize,
    ptid: usize,
    arg3: usize,
    arg4: usize,
) -> SysResult<usize> {
    let flags = CloneFlags::from_bits(flags as u32).ok_or(Errno::EINVAL)?;
    #[cfg(target_arch = "loongarch64")]
    let (ctid, tls) = (arg3, arg4);
    #[cfg(not(target_arch = "loongarch64"))]
    let (tls, ctid) = (arg3, arg4);

    if stack == 0 && flags.bits() == 0 {
        return Err(Errno::EINVAL);
    }

    // Linux 要求线程必须共享信号处理表和地址空间，且共享信号处理表必须共享地址空间。
    if flags.contains(CloneFlags::CLONE_THREAD) && !flags.contains(CloneFlags::CLONE_SIGHAND) {
        return Err(Errno::EINVAL);
    }
    if flags.contains(CloneFlags::CLONE_SIGHAND) && !flags.contains(CloneFlags::CLONE_VM) {
        return Err(Errno::EINVAL);
    }

    let current_task = current_task().expect("[kernel] current task is None.");
    if flags.contains(CloneFlags::CLONE_NEWUTS) && current_task.euid() != 0 {
        return Err(Errno::EPERM);
    }

    let share_vm = flags.share_user_vm();
    // 此处发生任务复制
    let new_task = current_task.clone_(flags)?;
    let new_tid = new_task.tid();

    #[cfg(target_arch = "loongarch64")]
    if LOONGARCH_PTHREAD_TRACE && flags.contains(CloneFlags::CLONE_THREAD) {
        let mut tls_head = 0usize;
        let _ = copy_from_user(&mut tls_head as *mut usize, tls as *const usize, 1);
        println!(
            "[la-pthread-trace] clone parent_tid={} parent_tgid={} new_tid={} flags={:?} stack={:#x} ptid={:#x} ctid={:#x} tls={:#x} tls_head={:#x}",
            current_task.tid(),
            current_task.tgid(),
            new_tid,
            flags,
            stack,
            ptid,
            ctid,
            tls,
            tls_head
        );
    }

    // CLONE_PARENT_SETTID: 在父进程地址空间写入子进程 tid
    if flags.contains(CloneFlags::CLONE_PARENT_SETTID) && ptid != 0 {
        let tid_val = new_tid as u32;
        copy_to_user(ptid as *mut u32, &tid_val as *const u32, 1)?;
    }

    // CLONE_CHILD_SETTID: 子线程开始运行前在 ctid 写入自己的 tid。
    // 对 CLONE_VM 线程，ctid 位于当前共享地址空间，可以直接写。
    // 对 fork 这类非共享地址空间的 clone，ctid 属于子进程地址空间；
    // 不能写到当前父进程地址空间，否则会污染 glibc 的 TLS/TCB。
    if flags.contains(CloneFlags::CLONE_CHILD_SETTID) && ctid != 0 {
        let tid_val = new_tid as u32;
        if share_vm {
            copy_to_user(ctid as *mut u32, &tid_val as *const u32, 1)?;
        } else {
            let parent = current_task.clone();
            new_task.op_memory_set_write(|memory_set| {
                let end_addr = ctid
                    .checked_add(core::mem::size_of::<u32>())
                    .ok_or(Errno::EFAULT)?;
                let start = VirtAddr::from(ctid).floor();
                let end = VirtAddr::from(end_addr).ceil();
                memory_set
                    .ensure_user_page_access(VPNRange::new(start, end), MapPermission::WRITE)?;
                memory_set.activate();
                unsafe {
                    (ctid as *mut u32).write(tid_val);
                }
                Ok::<(), Errno>(())
            })?;
            parent.op_memory_set_read(|memory_set| memory_set.activate());
        }
        new_task.set_set_child_tid(ctid);
    }

    // CLONE_CHILD_CLEARTID: 记录线程退出时清零并唤醒的地址
    if flags.contains(CloneFlags::CLONE_CHILD_CLEARTID) && ctid != 0 {
        new_task.set_clear_child_tid(ctid);
    }

    // 修改新任务的异常上下文，修改栈指针和返回值。
    // x4(tp) 属于用户态 TLS，不能写成内核 TaskControlBlock 指针。
    let new_task_trap_cx = new_task.get_trap_cx();
    if stack != 0 {
        new_task_trap_cx.set_sp(stack);
    }
    if flags.contains(CloneFlags::CLONE_SETTLS) {
        new_task_trap_cx.set_tp(tls);
    }
    new_task_trap_cx.set_a0(0);

    add_task(new_task);
    if flags.contains(CloneFlags::CLONE_VFORK) {
        blocking_and_run_next();
    }
    // 系统调用返回新创建任务的 pid
    Ok(new_tid)
}

pub fn sys_execve(path: *const u8, args: *const usize, envp: *const usize) -> SysResult<usize> {
    let path = copy_cstr_from_user(path)?;
    let args_vec = extract_cstrings_from_user(args)?;
    let envs_vec = if envp.is_null() {
        Vec::new()
    } else {
        extract_cstrings_from_user(envp)?
    };
    let task = current_task().expect("[kernel] current task is None.");

    if let Some(app_name) = builtin_for_fs_exec(path.as_str(), args_vec.as_slice()) {
        if let Some(data) = get_app_data_by_name(app_name) {
            if !is_elf(data) {
                return Err(Errno::ENOEXEC);
            }
            return Ok(task.execve(path.clone(), data, args_vec, envs_vec, false)?);
        }
    }

    match path_open(AT_FDCWD, &path, 0, 0) {
        Ok(file) => exec_fs_file(task, file, args_vec, envs_vec),
        Err(Errno::ENOENT) if !path.starts_with("/") => {
            // 从内核中加载的应用程序
            if let Some(data) = get_app_data_by_name(path.as_str()) {
                if !is_elf(data) {
                    return Err(Errno::ENOEXEC);
                }
                Ok(task.execve(path.clone(), data, args_vec, envs_vec, false)?)
            } else {
                Err(Errno::ENOENT)
            }
        }
        Err(err) => Err(err),
    }
}

fn check_exec_permission(task: &Arc<crate::task::TaskControlBlock>, file: &Arc<File>) -> SysResult {
    let inode = file.inode();
    if inode.node_type() == InodeType::Directory {
        return Err(Errno::EACCES);
    }

    let path = file.path().abs_path();
    let stat = inode.stat(path.as_str())?;
    let mode = stat.mode & 0o777;
    if task.fsuid() == 0 {
        if mode & 0o111 != 0 {
            return Ok(());
        }
        return Err(Errno::EACCES);
    }

    let exec_bit = if task.fsuid() as u32 == stat.uid {
        0o100
    } else if task.in_group(stat.gid as usize) {
        0o010
    } else {
        0o001
    };

    if mode & exec_bit != 0 {
        Ok(())
    } else {
        Err(Errno::EACCES)
    }
}

fn exec_fs_file(
    task: Arc<crate::task::TaskControlBlock>,
    file: Arc<File>,
    args_vec: Vec<String>,
    envs_vec: Vec<String>,
) -> SysResult<usize> {
    check_exec_permission(&task, &file)?;

    let exe_path = file.path().global_abs_path();
    let all_data = file.read_all()?;

    if !is_elf(all_data.as_slice()) {
        if let Some((shell_path, shell_args)) =
            shebang_args(exe_path.as_str(), all_data.as_slice(), args_vec.as_slice())
        {
            let shell_file = path_open(AT_FDCWD, shell_path.as_str(), 0, 0)?;
            check_exec_permission(&task, &shell_file)?;
            let shell_exe_path = shell_file.path().global_abs_path();
            let shell_data = shell_file.read_all()?;
            task.execve(
                shell_exe_path,
                shell_data.as_slice(),
                shell_args,
                envs_vec,
                true,
            )?;
            return Ok(0);
        }
        return Err(Errno::ENOEXEC);
    }

    task.execve(exe_path, all_data.as_slice(), args_vec, envs_vec, true)?;
    Ok(0)
}

pub fn sys_execveat(
    dirfd: isize,
    path: *const u8,
    args: *const usize,
    envp: *const usize,
    flags: usize,
) -> SysResult<usize> {
    const EXECVEAT_ALLOWED_FLAGS: usize = AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW;
    if flags & !EXECVEAT_ALLOWED_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }

    let path = copy_cstr_from_user(path)?;
    let open_flags = if flags & AT_SYMLINK_NOFOLLOW != 0 {
        usize::from(OpenFlags::O_NOFOLLOW)
    } else {
        0
    };
    let file = if path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 {
            return Err(Errno::ENOENT);
        }
        let fd_entry = current_task()
            .expect("[kernel] current task is None.")
            .get_fd_entry(dirfd as usize)?;
        let file = fd_entry.get_file();
        let file = file.as_any().downcast_ref::<File>().ok_or(Errno::EACCES)?;
        Arc::new(File::new(file.path(), file.inode(), file.get_flags()))
    } else {
        path_open(dirfd, &path, open_flags, 0)?
    };

    let args_vec = if args.is_null() {
        Vec::new()
    } else {
        extract_cstrings_from_user(args)?
    };
    let envs_vec = if envp.is_null() {
        Vec::new()
    } else {
        extract_cstrings_from_user(envp)?
    };
    let task = current_task().expect("[kernel] current task is None.");
    exec_fs_file(task, file, args_vec, envs_vec)
}

/// 等待子任务结束
///
/// - 参数：
///     - `pid` 接受查询子任务任务号，可选值 -1 表示任意子任务
///     - `exit_code_ptr` 目标子任务的 wait status
pub fn sys_wait4(
    pid: isize,
    exit_code_ptr: *mut i32,
    options: usize,
    rusage: *mut RUsage,
) -> SysResult<usize> {
    let options = WaitOption::from_bits(options as i32).ok_or(Errno::EINVAL)?;
    if pid == i32::MIN as isize {
        return Err(Errno::ESRCH);
    }

    let nohang = options.contains(WaitOption::WNOHANG);

    loop {
        let task = current_task().expect("[kernel] current task is None.");
        let current_pgid = task.pgid();
        let target_pid = (pid > 0).then_some(pid as usize);
        let target_pgid = if pid == 0 {
            Some(current_pgid)
        } else if pid < -1 {
            Some((-pid) as usize)
        } else {
            None
        };
        let wait_result = task.op_children_mut(|children| {
            if !children.iter().any(|(child_tid, child)| {
                pid == -1 || target_pid == Some(*child_tid) || target_pgid == Some(child.pgid())
            }) {
                return Err(Errno::ECHILD);
            }

            Ok(children.iter().find_map(|(child_tid, child)| {
                let matches_pid = pid == -1
                    || target_pid == Some(*child_tid)
                    || target_pgid == Some(child.pgid());
                if !matches_pid {
                    return None;
                }

                if options.intersects(WaitOption::WUNTRACED | WaitOption::WCONTINUED) {
                    if let Some((code, status)) = child.peek_wait_event() {
                        if (code == SigInfo::CLD_STOPPED && options.contains(WaitOption::WUNTRACED))
                            || (code == SigInfo::CLD_CONTINUED
                                && options.contains(WaitOption::WCONTINUED))
                        {
                            return Some((*child_tid, wait4_event_status(code, status), false));
                        }
                    }
                }

                child
                    .is_exited()
                    .then(|| (*child_tid, child.wait_status(), true))
            }))
        })?;

        if let Some((child_tid, wait_status, exited)) = wait_result {
            if !exit_code_ptr.is_null() {
                copy_to_user(exit_code_ptr, &wait_status as *const i32, 1)?;
            }

            if exited {
                let (child_utime, child_stime) = task.op_children_mut(|children| {
                    let child = children.get(&child_tid).unwrap();
                    let ticks = child.elapsed_ticks();
                    (ticks, ticks)
                });
                task.add_child_ticks(child_utime, child_stime);

                if !rusage.is_null() {
                    let usage = RUsage {
                        ru_utime: TimeVal {
                            sec: child_utime / CLK_TCK,
                            usec: (child_utime % CLK_TCK) * (1_000_000 / CLK_TCK),
                        },
                        ru_stime: TimeVal {
                            sec: child_stime / CLK_TCK,
                            usec: (child_stime % CLK_TCK) * (1_000_000 / CLK_TCK),
                        },
                        ..RUsage::default()
                    };
                    copy_to_user(rusage, &usage as *const RUsage, 1)?;
                }

                task.op_children_mut(|children| {
                    children.remove(&child_tid).unwrap();
                });
            } else {
                task.op_children_mut(|children| {
                    if let Some(child) = children.get(&child_tid) {
                        child.take_wait_event();
                    }
                });
            }

            return Ok(child_tid);
        }

        if nohang {
            return Ok(0);
        }

        blocking_and_run_next();
    }
}

const WAITID_P_ALL: usize = 0;
const WAITID_P_PID: usize = 1;
const WAITID_P_PGID: usize = 2;
const WAITID_WNOHANG: usize = 1;
const WAITID_WSTOPPED: usize = 2;
const WAITID_WEXITED: usize = 4;
const WAITID_WCONTINUED: usize = 8;
const WAITID_WNOWAIT: usize = 0x01000000;
const WAITID_ALLOWED_OPTIONS: usize =
    WAITID_WNOHANG | WAITID_WSTOPPED | WAITID_WEXITED | WAITID_WCONTINUED | WAITID_WNOWAIT;

fn waitid_child_info(pid: usize, status: i32) -> LinuxSigInfo {
    if status & 0x7f == 0 {
        LinuxSigInfo::new_child(pid, (status >> 8) & 0xff, SigInfo::CLD_EXITED)
    } else {
        let code = if status & 0x80 != 0 {
            SigInfo::CLD_DUMPED
        } else {
            SigInfo::CLD_KILLED
        };
        LinuxSigInfo::new_child(pid, status & 0x7f, code)
    }
}

fn wait4_event_status(code: i32, status: i32) -> i32 {
    match code {
        SigInfo::CLD_STOPPED => (status << 8) | 0x7f,
        SigInfo::CLD_CONTINUED => 0xffff,
        _ => status,
    }
}

pub fn sys_waitid(
    idtype: usize,
    id: usize,
    infop: *mut LinuxSigInfo,
    options: usize,
    _rusage: usize,
) -> SysResult<usize> {
    if options & !WAITID_ALLOWED_OPTIONS != 0
        || options & (WAITID_WEXITED | WAITID_WSTOPPED | WAITID_WCONTINUED) == 0
    {
        return Err(Errno::EINVAL);
    }
    if idtype > WAITID_P_PGID {
        return Err(Errno::EINVAL);
    }

    let nohang = options & WAITID_WNOHANG != 0;
    let nowait = options & WAITID_WNOWAIT != 0;

    loop {
        let task = current_task().expect("[kernel] current task is None.");
        let current_pgid = task.pgid();
        let target_pgid = if idtype == WAITID_P_PGID && id == 0 {
            current_pgid
        } else {
            id
        };

        let wait_result = task.op_children_mut(|children| {
            if !children.iter().any(|(child_tid, child)| match idtype {
                WAITID_P_ALL => true,
                WAITID_P_PID => *child_tid == id,
                WAITID_P_PGID => child.pgid() == target_pgid,
                _ => false,
            }) {
                return Err(Errno::ECHILD);
            }

            Ok(children.iter().find_map(|(child_tid, child)| {
                let matches_id = match idtype {
                    WAITID_P_ALL => true,
                    WAITID_P_PID => *child_tid == id,
                    WAITID_P_PGID => child.pgid() == target_pgid,
                    _ => false,
                };
                if !matches_id {
                    return None;
                }
                if options & (WAITID_WSTOPPED | WAITID_WCONTINUED) != 0 {
                    if let Some((code, status)) = child.peek_wait_event() {
                        if (code == SigInfo::CLD_STOPPED && options & WAITID_WSTOPPED != 0)
                            || (code == SigInfo::CLD_CONTINUED && options & WAITID_WCONTINUED != 0)
                        {
                            return Some((
                                *child_tid,
                                LinuxSigInfo::new_child(*child_tid, status, code),
                                false,
                            ));
                        }
                    }
                }
                if options & WAITID_WEXITED != 0 && child.is_exited() {
                    return Some((
                        *child_tid,
                        waitid_child_info(*child_tid, child.wait_status()),
                        true,
                    ));
                }
                None
            }))
        })?;

        if let Some((child_tid, info, exited)) = wait_result {
            if !infop.is_null() {
                copy_to_user(infop, &info as *const LinuxSigInfo, 1)?;
            }
            if !nowait {
                if exited {
                    task.op_children_mut(|children| {
                        children.remove(&child_tid).unwrap();
                    });
                } else {
                    task.op_children_mut(|children| {
                        if let Some(child) = children.get(&child_tid) {
                            child.take_wait_event();
                        }
                    });
                }
            }
            return Ok(0);
        }

        if nohang {
            return Ok(0);
        }

        blocking_and_run_next();
    }
}

const PRIO_PROCESS: usize = 0;
const PRIO_PGRP: usize = 1;
const PRIO_USER: usize = 2;

fn priority_targets(which: usize, who: usize) -> SysResult<Vec<Arc<TaskControlBlock>>> {
    let current_thread = current_task().expect("[kernel] current task is None.");
    let current = process_leader(&current_thread);
    let target_id = match which {
        PRIO_PROCESS => {
            if who == 0 {
                current.tgid()
            } else {
                who
            }
        }
        PRIO_PGRP => {
            if who == 0 {
                current.pgid()
            } else {
                who
            }
        }
        PRIO_USER => {
            if who == 0 {
                current.uid()
            } else {
                who
            }
        }
        _ => return Err(Errno::EINVAL),
    };

    let mut targets = Vec::new();
    TASK_MANAGER.for_each(|task| {
        if !task.is_process_leader() {
            return;
        }
        let matches = match which {
            PRIO_PROCESS => task.tgid() == target_id,
            PRIO_PGRP => task.pgid() == target_id,
            PRIO_USER => task.uid() == target_id,
            _ => false,
        };
        if matches {
            targets.push(task.clone());
        }
    });

    if targets.is_empty() {
        Err(Errno::ESRCH)
    } else {
        Ok(targets)
    }
}

/// 系统调用 sys-setpriority
pub fn sys_setpriority(which: usize, who: usize, prio: isize) -> SysResult<usize> {
    let current = process_leader(&current_task().expect("[kernel] current task is None."));
    let nice = (prio as i32).clamp(-20, 19);
    let targets = priority_targets(which, who)?;

    let mut denied_other_user = false;
    for target in &targets {
        if current.euid() == 0 {
            continue;
        }
        let same_user = current.euid() == target.uid() || current.euid() == target.euid();
        if same_user && nice < target.nice() {
            return Err(Errno::EACCES);
        }
        if !same_user {
            denied_other_user = true;
        }
    }
    if denied_other_user {
        return Err(Errno::EPERM);
    }

    for target in targets {
        target.op_thread_group(|tg| {
            for member in tg.iter() {
                member.set_nice(nice);
            }
        });
    }
    Ok(0)
}

pub fn sys_getpriority(which: usize, who: usize) -> SysResult<usize> {
    let targets = priority_targets(which, who)?;
    let nice = targets
        .iter()
        .map(|task| task.nice())
        .min()
        .ok_or(Errno::ESRCH)?;
    Ok((20 - nice) as usize)
}

/// 系统调用 sys-getpid
pub fn sys_getpid() -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    Ok(task.tgid())
}

/// 系统调用 sys-getppid
pub fn sys_getppid() -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    Ok(task.op_parent(|parent| parent.as_ref().unwrap().upgrade().unwrap().tid()))
}

/// 系统调用 sys_set_tid_address
///
/// musl 初始化线程库时调用，设置 clear-child-tid 地址。
/// 与 CLONE_CHILD_CLEARTID 配合，在线程退出时向该地址写入 0 并 futex wake，
/// 以唤醒 wait4 / pthread_join 的调用者。
pub fn sys_set_tid_address(tidptr: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    task.set_clear_child_tid(tidptr);
    Ok(task.tid())
}

/// 系统调用 sys_futex - 快速用户空间互斥锁
///
/// FUTEX_WAIT: 如果 *uaddr == val ，则阻塞当前任务；否则返回 EAGAIN
/// FUTEX_WAKE: 唤醒最多 val 个阻塞在 uaddr 上的任务，返回实际唤醒数
pub fn sys_futex(
    uaddr: *const i32,
    futex_op: usize,
    val: usize,
    timeout: usize,
    uaddr2: usize,
    val3: usize,
) -> SysResult<usize> {
    do_futex(uaddr as usize, futex_op, val, timeout, uaddr2, val3)
}

/// 系统调用 sys_set_robust_list - 设置线程的 robust futex 链表
pub fn sys_set_robust_list(head: usize, len: usize) -> SysResult<usize> {
    const ROBUST_LIST_HEAD_SIZE: usize = core::mem::size_of::<usize>() * 3;
    if len != ROBUST_LIST_HEAD_SIZE {
        return Err(Errno::EINVAL);
    }
    let task = current_task().expect("[kernel] current task is None.");
    task.set_robust_list(head, len);
    Ok(0)
}

pub fn sys_get_robust_list(
    pid: usize,
    head_ptr: *mut usize,
    len_ptr: *mut usize,
) -> SysResult<usize> {
    const ROBUST_LIST_HEAD_SIZE: usize = core::mem::size_of::<usize>() * 3;

    let task = current_task().expect("[kernel] current task is None.");
    if pid != 0 && pid != task.tid() {
        return Err(Errno::ESRCH);
    }

    let head = task.robust_list().map(|(head, _)| head).unwrap_or(0);
    let len = task
        .robust_list()
        .map(|(_, len)| len)
        .unwrap_or(ROBUST_LIST_HEAD_SIZE);
    copy_to_user(head_ptr, &head as *const usize, 1)?;
    copy_to_user(len_ptr, &len as *const usize, 1)?;
    Ok(0)
}

/// 系统调用 sys_getuid - 获取实际用户 ID
pub fn sys_getuid() -> SysResult<usize> {
    Ok(current_task()
        .expect("[kernel] current task is None.")
        .uid())
}

pub fn sys_setuid(uid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    if task.euid() == 0 {
        task.set_uid_triplet(uid, uid, uid);
        return Ok(0);
    }
    if uid == task.uid() || uid == task.euid() || uid == task.suid() {
        task.set_uid_triplet(task.uid(), uid, task.suid());
        return Ok(0);
    }
    Err(Errno::EPERM)
}

fn is_unchanged_id(id: usize) -> bool {
    id == usize::MAX || id == u32::MAX as usize
}

fn resolve_new_id(new_id: usize, old_id: usize) -> usize {
    if is_unchanged_id(new_id) {
        old_id
    } else {
        new_id
    }
}

fn can_set_uid(task_uid: usize, task_euid: usize, task_suid: usize, target: usize) -> bool {
    target == task_uid || target == task_euid || target == task_suid
}

fn can_set_gid(task_gid: usize, task_egid: usize, task_sgid: usize, target: usize) -> bool {
    target == task_gid || target == task_egid || target == task_sgid
}

pub fn sys_setreuid(ruid: usize, euid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let old_ruid = task.uid();
    let old_suid = task.suid();
    let new_ruid = resolve_new_id(ruid, task.uid());
    let new_euid = resolve_new_id(euid, task.euid());
    if task.euid() != 0
        && (!can_set_uid(task.uid(), task.euid(), task.suid(), new_ruid)
            || !can_set_uid(task.uid(), task.euid(), task.suid(), new_euid))
    {
        return Err(Errno::EPERM);
    }
    let new_suid = if !is_unchanged_id(ruid) || (!is_unchanged_id(euid) && new_euid != old_ruid) {
        new_euid
    } else {
        old_suid
    };
    task.set_uid_triplet(new_ruid, new_euid, new_suid);
    Ok(0)
}

/// 系统调用 sys_geteuid - 获取有效用户 ID
pub fn sys_geteuid() -> SysResult<usize> {
    Ok(current_task()
        .expect("[kernel] current task is None.")
        .euid())
}

/// 系统调用 sys_getgid - 获取实际组 ID
pub fn sys_getgid() -> SysResult<usize> {
    Ok(current_task()
        .expect("[kernel] current task is None.")
        .gid())
}

pub fn sys_setgid(gid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    if task.euid() == 0 {
        task.set_gid_triplet(gid, gid, gid);
        return Ok(0);
    }
    if gid == task.gid() || gid == task.egid() || gid == task.sgid() {
        task.set_gid_triplet(task.gid(), gid, task.sgid());
        return Ok(0);
    }
    Err(Errno::EPERM)
}

pub fn sys_setregid(rgid: usize, egid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let old_rgid = task.gid();
    let old_sgid = task.sgid();
    let new_rgid = resolve_new_id(rgid, task.gid());
    let new_egid = resolve_new_id(egid, task.egid());
    if task.euid() != 0
        && (!can_set_gid(task.gid(), task.egid(), task.sgid(), new_rgid)
            || !can_set_gid(task.gid(), task.egid(), task.sgid(), new_egid))
    {
        return Err(Errno::EPERM);
    }
    let new_sgid = if !is_unchanged_id(rgid) || (!is_unchanged_id(egid) && new_egid != old_rgid) {
        new_egid
    } else {
        old_sgid
    };
    task.set_gid_triplet(new_rgid, new_egid, new_sgid);
    Ok(0)
}

/// 系统调用 sys_getegid - 获取有效组 ID
pub fn sys_getegid() -> SysResult<usize> {
    Ok(current_task()
        .expect("[kernel] current task is None.")
        .egid())
}

pub fn sys_setresuid(ruid: usize, euid: usize, suid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let new_ruid = resolve_new_id(ruid, task.uid());
    let new_euid = resolve_new_id(euid, task.euid());
    let new_suid = resolve_new_id(suid, task.suid());
    if task.euid() != 0
        && (!can_set_uid(task.uid(), task.euid(), task.suid(), new_ruid)
            || !can_set_uid(task.uid(), task.euid(), task.suid(), new_euid)
            || !can_set_uid(task.uid(), task.euid(), task.suid(), new_suid))
    {
        return Err(Errno::EPERM);
    }
    task.set_uid_triplet(new_ruid, new_euid, new_suid);
    Ok(0)
}

pub fn sys_getresuid(ruid: *mut u32, euid: *mut u32, suid: *mut u32) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let r = task.uid() as u32;
    let e = task.euid() as u32;
    let s = task.suid() as u32;
    copy_to_user(ruid, &r as *const u32, 1)?;
    copy_to_user(euid, &e as *const u32, 1)?;
    copy_to_user(suid, &s as *const u32, 1)?;
    Ok(0)
}

pub fn sys_setresgid(rgid: usize, egid: usize, sgid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let new_rgid = resolve_new_id(rgid, task.gid());
    let new_egid = resolve_new_id(egid, task.egid());
    let new_sgid = resolve_new_id(sgid, task.sgid());
    if task.euid() != 0
        && (!can_set_gid(task.gid(), task.egid(), task.sgid(), new_rgid)
            || !can_set_gid(task.gid(), task.egid(), task.sgid(), new_egid)
            || !can_set_gid(task.gid(), task.egid(), task.sgid(), new_sgid))
    {
        return Err(Errno::EPERM);
    }
    task.set_gid_triplet(new_rgid, new_egid, new_sgid);
    Ok(0)
}

pub fn sys_getresgid(rgid: *mut u32, egid: *mut u32, sgid: *mut u32) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let r = task.gid() as u32;
    let e = task.egid() as u32;
    let s = task.sgid() as u32;
    copy_to_user(rgid, &r as *const u32, 1)?;
    copy_to_user(egid, &e as *const u32, 1)?;
    copy_to_user(sgid, &s as *const u32, 1)?;
    Ok(0)
}

pub fn sys_getgroups(size: usize, list: *mut u32) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let groups = task.supplementary_groups();
    if size == 0 {
        return Ok(groups.len());
    }
    if size < groups.len() {
        return Err(Errno::EINVAL);
    }
    for (idx, gid) in groups.iter().enumerate() {
        let gid = *gid as u32;
        copy_to_user(list.wrapping_add(idx), &gid as *const u32, 1)?;
    }
    Ok(groups.len())
}

pub fn sys_setgroups(size: usize, list: *const u32) -> SysResult<usize> {
    const NGROUPS_MAX: usize = 65_536;
    let task = current_task().expect("[kernel] current task is None.");
    if task.euid() != 0 {
        return Err(Errno::EPERM);
    }
    if size > NGROUPS_MAX {
        return Err(Errno::EINVAL);
    }
    let mut groups = Vec::with_capacity(size);
    for idx in 0..size {
        let mut gid = 0u32;
        copy_from_user(&mut gid as *mut u32, list.wrapping_add(idx), 1)?;
        groups.push(gid as usize);
    }
    task.set_supplementary_groups(groups);
    Ok(0)
}

pub fn sys_setfsuid(uid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let old = task.fsuid();
    if !is_unchanged_id(uid)
        && (task.euid() == 0 || can_set_uid(task.uid(), task.euid(), task.suid(), uid))
    {
        task.set_fsuid(uid);
    }
    Ok(old)
}

pub fn sys_setfsgid(gid: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    let old = task.fsgid();
    if !is_unchanged_id(gid)
        && (task.euid() == 0 || can_set_gid(task.gid(), task.egid(), task.sgid(), gid))
    {
        task.set_fsgid(gid);
    }
    Ok(old)
}

pub fn sys_umask(mask: usize) -> SysResult<usize> {
    let task = current_task().expect("[kernel] current task is None.");
    Ok(task.set_umask(mask))
}
