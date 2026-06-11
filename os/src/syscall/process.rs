// os/src/syscall/process.rs

use super::time::TimeVal;
use super::{Errno, SysResult};
use crate::config::USER_STACK_SIZE;
use crate::fs::{AT_FDCWD, path_open};
use crate::loader::get_app_data_by_name;
use crate::mm::{
    MapPermission, VPNRange, VirtAddr, copy_cstr_from_user, copy_from_user, copy_to_user,
    extract_cstrings_from_user,
};
use crate::task::{
    CloneFlags, WaitOption, add_task, current_task, do_futex, exit_and_run_next,
    exit_group_and_run_next, yield_current_task,
};
use alloc::string::String;
use alloc::vec::Vec;

#[cfg(target_arch = "loongarch64")]
const LOONGARCH_PTHREAD_TRACE: bool = false;

fn is_elf(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == [0x7f, b'E', b'L', b'F']
}

fn shebang_busybox_path(script_path: &str) -> &'static str {
    if script_path.starts_with("/glibc/") {
        "/glibc/busybox"
    } else {
        "/musl/busybox"
    }
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

pub fn sys_gettid() -> SysResult<usize> {
    Ok(current_task()
        .expect("[kernel] current task is None.")
        .tid())
}

pub fn sys_prlimit64(
    pid: usize,
    resource: usize,
    new_limit: *const RLimit,
    old_limit: *mut RLimit,
) -> SysResult<usize> {
    const RLIMIT_NOFILE: usize = 7;
    const RLIMIT_STACK: usize = 3;
    const RLIM_INFINITY: usize = usize::MAX;

    let task = current_task().expect("[kernel] current task is None.");
    if pid != 0 && pid != task.tid() {
        return Err(Errno::ESRCH);
    }

    let old = match resource {
        RLIMIT_NOFILE => {
            let (cur, max) = task.nofile_limit();
            RLimit { cur, max }
        }
        RLIMIT_STACK => RLimit {
            cur: USER_STACK_SIZE,
            max: RLIM_INFINITY,
        },
        _ => RLimit {
            cur: RLIM_INFINITY,
            max: RLIM_INFINITY,
        },
    };

    if !new_limit.is_null() {
        let mut limit = RLimit { cur: 0, max: 0 };
        copy_from_user(&mut limit as *mut RLimit, new_limit, 1)?;
        if resource == RLIMIT_NOFILE {
            task.set_nofile_limit(limit.cur, limit.max)?;
        }
    }

    if !old_limit.is_null() {
        copy_to_user(old_limit, &old as *const RLimit, 1)?;
    }

    Ok(0)
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

    // 简化模型：CLONE_THREAD 表示真正线程，必须共享地址空间。
    // 不共享地址空间的可调度实体按新进程处理，而不是放进同一线程组。
    if flags.contains(CloneFlags::CLONE_THREAD) && !flags.contains(CloneFlags::CLONE_VM) {
        return Err(Errno::EINVAL);
    }
    if flags.contains(CloneFlags::CLONE_SIGHAND) && !flags.contains(CloneFlags::CLONE_VM) {
        return Err(Errno::EINVAL);
    }

    let current_task = current_task().expect("[kernel] current task is None.");
    // 此处发生任务复制
    let new_task = current_task.clone_(flags);
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
        if flags.contains(CloneFlags::CLONE_VM) {
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

    if let Ok(file) = path_open(AT_FDCWD, &path, 0, 0) {
        // info!("[kernel] execute file in fs");
        let exe_path = file.path().global_abs_path();
        let all_data = file.read_all()?;

        if !is_elf(all_data.as_slice()) {
            if let Some((shell_path, shell_args)) =
                shebang_args(exe_path.as_str(), all_data.as_slice(), args_vec.as_slice())
            {
                let shell_file = path_open(AT_FDCWD, shell_path.as_str(), 0, 0)?;
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
    } else if !path.starts_with("/") {
        // 从内核中加载的应用程序
        if let Some(data) = get_app_data_by_name(path.as_str()) {
            if !is_elf(data) {
                return Err(Errno::ENOEXEC);
            }
            Ok(task.execve(path.clone(), data, args_vec, envs_vec, false)?)
        } else {
            Err(Errno::ENOENT)
        }
    } else {
        Err(Errno::ENOENT)
    }
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

    // pid == 0 和 pid < -1 需要按进程组等待；当前任务结构尚未维护 pgid，
    // 先显式拒绝，避免把进程组语义错误地退化成等待任意子任务。
    if pid == 0 || pid < -1 {
        return Err(Errno::EINVAL);
    }

    let nohang = options.contains(WaitOption::WNOHANG);

    loop {
        let task = current_task().expect("[kernel] current task is None.");
        let wait_result = task.op_children_mut(|children| {
            let matches_pid = |child_tid: usize| pid == -1 || pid as usize == child_tid;

            if !children.keys().any(|child_tid| matches_pid(*child_tid)) {
                return Err(Errno::ECHILD);
            }

            Ok(children
                .iter()
                .find(|(child_tid, child)| matches_pid(**child_tid) && child.is_exited())
                .map(|(child_tid, child)| (*child_tid, child.wait_status())))
        })?;

        if let Some((child_tid, wait_status)) = wait_result {
            if !exit_code_ptr.is_null() {
                copy_to_user(exit_code_ptr, &wait_status as *const i32, 1)?;
            }

            // 当前内核还没有任务资源统计，先按 wait4 ABI 写回零值结构。
            if !rusage.is_null() {
                let usage = RUsage::default();
                copy_to_user(rusage, &usage as *const RUsage, 1)?;
            }

            task.op_children_mut(|children| {
                children.remove(&child_tid).unwrap();
            });

            warn! {"[kernel] (wait4) parent:{}, child:{}.", task.tid(), child_tid};

            return Ok(child_tid);
        }

        if nohang {
            return Ok(0);
        }

        yield_current_task();
    }
}

/// 系统调用 sys-setpriority
/// TODO[UNIMPLEMENTED]: 需要补完 setpriority 逻辑。
pub fn sys_setpriority(which: usize, who: usize, prio: isize) -> SysResult<usize> {
    let _ = (which, who, prio);
    Err(Errno::ENOSYS)
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
    Ok(0)
}

/// 系统调用 sys_geteuid - 获取有效用户 ID
pub fn sys_geteuid() -> SysResult<usize> {
    Ok(0)
}

/// 系统调用 sys_getgid - 获取实际组 ID
pub fn sys_getgid() -> SysResult<usize> {
    Ok(0)
}

/// 系统调用 sys_getegid - 获取有效组 ID
pub fn sys_getegid() -> SysResult<usize> {
    Ok(0)
}
