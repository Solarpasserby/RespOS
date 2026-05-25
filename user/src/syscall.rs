use core::arch::asm;
use crate::SignalAction;

const SYSCALL_GETCWD: usize     = 17;
const SYSCALL_DUP: usize        = 23;
const SYSCALL_DUP3: usize       = 24;
const SYSCALL_MKDIRAT: usize    = 34;
const SYSCALL_UNLINKAT: usize   = 35;
const SYSCALL_LINKAT: usize     = 37;
const SYSCALL_UMOUNT2: usize    = 39;
const SYSCALL_MOUNT: usize      = 40;
const SYSCALL_CHDIR: usize      = 49;
const SYSCALL_OPENAT: usize     = 56;
const SYSCALL_CLOSE: usize      = 57;
const SYSCALL_PIPE2: usize      = 59;
const SYSCALL_GETDENTS64: usize = 61;
const SYSCALL_LSEEK: usize      = 62;
const SYSCALL_READ: usize       = 63;
const SYSCALL_WRITE: usize      = 64;
const SYSCALL_STAT: usize       = 79;
const SYSCALL_FSTAT: usize      = 80;
const SYSCALL_EXIT: usize       = 93;
const SYSCALL_NANOSLEEP: usize  = 101;
const SYSCALL_SCHED_YIELD: usize = 124;
const SYSCALL_SETPRIORITY: usize = 140;
const SYSCALL_TIMES: usize      = 153;
const SYSCALL_UNAME: usize      = 160;
const SYSCALL_KILL: usize     = 129;
const SYSCALL_SIGACTION: usize = 134;
const SYSCALL_SIGPROCMASK: usize = 135;
const SYSCALL_SIGRETURN: usize = 139;
const SYSCALL_GETTIMEOFDAY: usize = 169;
const SYSCALL_GETPID: usize     = 172;
const SYSCALL_GETPPID: usize    = 173;
const SYSCALL_BRK: usize        = 214;
const SYSCALL_MUNMAP: usize     = 215;
const SYSCALL_CLONE: usize      = 220;
const SYSCALL_EXECVE: usize     = 221;
const SYSCALL_MMAP: usize       = 222;
const SYSCALL_WAIT4: usize      = 260;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct TimeSpec {
    pub sec: usize,
    pub nsec: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Tms {
    pub tms_utime: usize,
    pub tms_stime: usize,
    pub tms_cutime: usize,
    pub tms_cstime: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub __pad: u64,
    pub st_size: u64,
    pub st_blksize: u32,
    pub __pad2: u32,
    pub st_blocks: u64,
    pub st_atime: TimeSpec,
    pub st_mtime: TimeSpec,
    pub st_ctime: TimeSpec,
    pub unused: u64,
}

fn syscall(id: usize, args: [usize; 6]) -> isize {
    let mut ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") args[0] => ret,
            in("a1") args[1],
            in("a2") args[2],
            in("a3") args[3],
            in("a4") args[4],
            in("a5") args[5],
            in("a7") id
        );
    }
    ret
}

pub fn sys_read(fd: usize, buf: &mut[u8]) -> isize {
    syscall(SYSCALL_READ, [fd, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0])
}

pub fn sys_write(fd: usize, buf: &[u8]) -> isize {
    syscall(SYSCALL_WRITE, [fd, buf.as_ptr() as usize, buf.len(), 0, 0, 0])
}

pub fn sys_getcwd(buf: &mut [u8]) -> isize {
    syscall(SYSCALL_GETCWD, [buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0, 0])
}

pub fn sys_dup(fd: usize) -> isize {
    syscall(SYSCALL_DUP, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_dup3(fd_src: usize, fd_dst: usize, flags: usize) -> isize {
    syscall(SYSCALL_DUP3, [fd_src, fd_dst, flags, 0, 0, 0])
}

pub fn sys_mkdirat(dirfd: isize, path: &str, mode: usize) -> isize {
    syscall(SYSCALL_MKDIRAT, [dirfd as usize, path.as_ptr() as usize, mode, 0, 0, 0])
}

pub fn sys_unlinkat(dirfd: isize, path: &str, flags: usize) -> isize {
    syscall(SYSCALL_UNLINKAT, [dirfd as usize, path.as_ptr() as usize, flags, 0, 0, 0])
}

pub fn sys_chdir(path: &str) -> isize {
    syscall(SYSCALL_CHDIR, [path.as_ptr() as usize, 0, 0, 0, 0, 0])
}

pub fn sys_openat(dirfd: isize, path: &str, flags: usize, mode: usize) -> isize {
    syscall(SYSCALL_OPENAT, [dirfd as usize, path.as_ptr() as usize, flags, mode, 0, 0])
}

pub fn sys_close(fd: usize) -> isize {
    syscall(SYSCALL_CLOSE, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_pipe2(pipefd: &mut [usize; 2], flags: usize) -> isize {
    syscall(SYSCALL_PIPE2, [pipefd.as_mut_ptr() as usize, flags, 0, 0, 0, 0])
}

pub fn sys_getdents64(fd: usize, dirp: *mut u8, count: usize) -> isize {
    syscall(SYSCALL_GETDENTS64, [fd, dirp as usize, count, 0, 0, 0])
}

pub fn sys_lseek(fd: usize, offset: isize, whence: usize) -> isize {
    syscall(SYSCALL_LSEEK, [fd, offset as usize, whence, 0, 0, 0])
}

pub fn sys_stat(path: &str, stat: &mut Stat) -> isize {
    syscall(SYSCALL_STAT, [path.as_ptr() as usize, stat as *mut _ as usize, 0, 0, 0, 0])
}

pub fn sys_fstat(fd: usize, stat: &mut Stat) -> isize {
    syscall(SYSCALL_FSTAT, [fd, stat as *mut _ as usize, 0, 0, 0, 0])
}

pub fn sys_exit(exit_code: i32) -> isize {
    syscall(SYSCALL_EXIT, [exit_code as usize, 0, 0, 0, 0, 0])
}

/// 主动交出 CPU 所有权
pub fn sys_sched_yield() -> isize {
    syscall(SYSCALL_SCHED_YIELD, [0, 0, 0, 0, 0, 0])
}

pub fn sys_gettimeofday(tv: &mut TimeVal, tz: usize) -> isize {
    syscall(SYSCALL_GETTIMEOFDAY, [tv as *mut _ as usize, tz, 0, 0, 0, 0])
}

pub fn sys_clone(flags: usize, stack: usize, ptid: usize, tls: usize, ctid: usize) -> isize {
    syscall(SYSCALL_CLONE, [flags, stack, ptid, tls, ctid, 0])
}

pub fn sys_execve(path: &str, args: &[*const u8], envp: *const *const u8) -> isize {
    syscall(SYSCALL_EXECVE, [path.as_ptr() as usize, args.as_ptr() as usize, envp as usize, 0, 0, 0])
}

pub fn sys_wait4(pid: isize, exit_code: *mut i32) -> isize {
    syscall(SYSCALL_WAIT4, [pid as usize, exit_code as usize, 0, 0, 0, 0])
}

pub fn sys_kill(pid: usize, signum: i32) -> isize {
    syscall(SYSCALL_KILL, [pid, signum as usize, 0, 0, 0, 0])
}     

pub fn sys_sigaction(
    signum: i32,
    action: *const SignalAction,
    old_action: *mut SignalAction,
) -> isize {
    syscall(
        SYSCALL_SIGACTION,
        [signum as usize, action as usize, old_action as usize, 0, 0, 0],
    )
    /*
    syscall(
        SYSCALL_SIGACTION,
        [
            signum as usize,
            action.map_or(0, |r| r as *const _ as usize),
            old_action.map_or(0, |r| r as *mut _ as usize),
        ],
    )
    */
}

pub fn sys_sigprocmask(mask: u32) -> isize {
    syscall(SYSCALL_SIGPROCMASK, [mask as usize, 0, 0, 0, 0, 0])
}

pub fn sys_sigreturn() -> isize {
    syscall(SYSCALL_SIGRETURN, [0, 0, 0, 0, 0, 0])
}

pub fn sys_linkat(
    olddirfd: isize,
    oldpath: &str,
    newdirfd: isize,
    newpath: &str,
    flags: usize,
) -> isize {
    syscall(SYSCALL_LINKAT, [
        olddirfd as usize,
        oldpath.as_ptr() as usize,
        newdirfd as usize,
        newpath.as_ptr() as usize,
        flags,
        0,
    ])
}

pub fn sys_mount(
    source: &str,
    target: &str,
    fstype: &str,
    flags: usize,
    data: usize,
) -> isize {
    syscall(SYSCALL_MOUNT, [
        source.as_ptr() as usize,
        target.as_ptr() as usize,
        fstype.as_ptr() as usize,
        flags,
        data,
        0,
    ])
}

pub fn sys_umount2(target: &str, flags: usize) -> isize {
    syscall(SYSCALL_UMOUNT2, [target.as_ptr() as usize, flags, 0, 0, 0, 0])
}

pub fn sys_nanosleep(req: &TimeVal, rem: &mut TimeVal) -> isize {
    syscall(SYSCALL_NANOSLEEP, [req as *const _ as usize, rem as *mut _ as usize, 0, 0, 0, 0])
}

pub fn sys_setpriority(which: usize, who: usize, prio: isize) -> isize {
    syscall(SYSCALL_SETPRIORITY, [which, who, prio as usize, 0, 0, 0])
}

pub fn sys_times(tms: &mut Tms) -> isize {
    syscall(SYSCALL_TIMES, [tms as *mut _ as usize, 0, 0, 0, 0, 0])
}

pub fn sys_uname(buf: &mut UtsName) -> isize {
    syscall(SYSCALL_UNAME, [buf as *mut _ as usize, 0, 0, 0, 0, 0])
}

pub fn sys_getpid() -> isize {
    syscall(SYSCALL_GETPID, [0, 0, 0, 0, 0, 0])
}

pub fn sys_getppid() -> isize {
    syscall(SYSCALL_GETPPID, [0, 0, 0, 0, 0, 0])
}

pub fn sys_brk(addr: usize) -> isize {
    syscall(SYSCALL_BRK, [addr, 0, 0, 0, 0, 0])
}

pub fn sys_munmap(addr: usize, len: usize) -> isize {
    syscall(SYSCALL_MUNMAP, [addr, len, 0, 0, 0, 0])
}

pub fn sys_mmap(
    addr: usize,
    len: usize,
    prot: usize,
    flags: usize,
    fd: isize,
    offset: usize,
) -> isize {
    syscall(SYSCALL_MMAP, [addr, len, prot, flags, fd as usize, offset])
}
