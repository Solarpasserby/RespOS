use core::arch::asm;
use crate::SignalAction;

const SYSCALL_GETCWD: usize   = 17;
const SYSCALL_DUP: usize      = 23;
const SYSCALL_DUP2: usize     = 24;
const SYSCALL_MKDIR: usize    = 34;
const SYSCALL_UNLINK: usize   = 35;
const SYSCALL_CHDIR: usize    = 49;
const SYSCALL_OPEN: usize     = 56;
const SYSCALL_CLOSE: usize    = 57;
const SYSCALL_PIPE: usize     = 59;
const SYSCALL_LSEEK: usize    = 62;
const SYSCALL_READ: usize     = 63;
const SYSCALL_WRITE: usize    = 64;
const SYSCALL_STAT: usize     = 79;
const SYSCALL_FSTAT: usize    = 80;
const SYSCALL_EXIT: usize     = 93;
const SYSCALL_YIELD: usize    = 124;
const SYSCALL_KILL: usize     = 129;
const SYSCALL_SIGACTION: usize = 134;
const SYSCALL_SIGPROCMASK: usize = 135;
const SYSCALL_SIGRETURN: usize = 139;
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_FORK: usize     = 220;
const SYSCALL_EXEC: usize     = 221;
const SYSCALL_WAITPID: usize  = 260;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct TimeSpec {
    pub sec: usize,
    pub nsec: usize,
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

fn syscall(id: usize, args: [usize; 3]) -> isize {
    let mut ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") args[0] => ret,
            in("a1") args[1],
            in("a2") args[2],
            in("a7") id
        );
    }
    ret
}

pub fn sys_read(fd: usize, buf: &mut[u8]) -> isize {
    syscall(SYSCALL_READ, [fd, buf.as_mut_ptr() as usize, buf.len()])
}

pub fn sys_write(fd: usize, buf: &[u8]) -> isize {
    syscall(SYSCALL_WRITE, [fd, buf.as_ptr() as usize, buf.len()])
}

pub fn sys_getcwd(buf: &mut [u8]) -> isize {
    syscall(SYSCALL_GETCWD, [buf.as_mut_ptr() as usize, buf.len(), 0])
}

pub fn sys_dup(fd: usize) -> isize {
    syscall(SYSCALL_DUP, [fd, 0, 0])
}

pub fn sys_dup2(fd_src: usize, fd_dst: usize) -> isize {
    syscall(SYSCALL_DUP2, [fd_src, fd_dst, 0])
}

pub fn sys_mkdir(path: &str, mode: usize) -> isize {
    syscall(SYSCALL_MKDIR, [path.as_ptr() as usize, mode, 0])
}

pub fn sys_unlink(path: &str) -> isize {
    syscall(SYSCALL_UNLINK, [path.as_ptr() as usize, 0, 0])
}

pub fn sys_chdir(path: &str) -> isize {
    syscall(SYSCALL_CHDIR, [path.as_ptr() as usize, 0, 0])
}

pub fn sys_open(path: &str, flags: usize, mode: usize) -> isize {
    syscall(SYSCALL_OPEN, [path.as_ptr() as usize, flags, mode])
}

pub fn sys_close(fd: usize) -> isize {
    syscall(SYSCALL_CLOSE, [fd, 0, 0])
}

pub fn sys_pipe(pipefd: &mut [u32; 2]) -> isize {
    syscall(SYSCALL_PIPE, [pipefd.as_mut_ptr() as usize, 0, 0])
}

pub fn sys_lseek(fd: usize, offset: isize, whence: usize) -> isize {
    syscall(SYSCALL_LSEEK, [fd, offset as usize, whence])
}

pub fn sys_stat(path: &str, stat: &mut Stat) -> isize {
    syscall(SYSCALL_STAT, [path.as_ptr() as usize, stat as *mut _ as usize, 0])
}

pub fn sys_fstat(fd: usize, stat: &mut Stat) -> isize {
    syscall(SYSCALL_FSTAT, [fd, stat as *mut _ as usize, 0])
}

pub fn sys_exit(exit_code: i32) -> isize {
    syscall(SYSCALL_EXIT, [exit_code as usize, 0, 0])
}

/// 主动交出 CPU 所有权
pub fn sys_yield() -> isize {
    syscall(SYSCALL_YIELD, [0, 0, 0])
}

pub fn sys_get_time() -> isize {
    syscall(SYSCALL_GET_TIME, [0, 0, 0])
}

pub fn sys_fork() -> isize {
    syscall(SYSCALL_FORK, [0, 0, 0])
}

pub fn sys_exec(path: &str) -> isize {
    syscall(SYSCALL_EXEC, [path.as_ptr() as usize, 0, 0])
}

pub fn sys_waitpid(pid: isize, exit_code: *mut i32) -> isize {
    syscall(SYSCALL_WAITPID, [pid as usize, exit_code as usize, 0])
}

pub fn sys_kill(pid: usize, signum: i32) -> isize {
    syscall(SYSCALL_KILL, [pid, signum as usize, 0])
}     

pub fn sys_sigaction(
    signum: i32,
    action: *const SignalAction,
    old_action: *mut SignalAction,
) -> isize {
    syscall(
        SYSCALL_SIGACTION,
        [signum as usize, action as usize, old_action as usize],
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
    syscall(SYSCALL_SIGPROCMASK, [mask as usize, 0, 0])
}

pub fn sys_sigreturn() -> isize {
    syscall(SYSCALL_SIGRETURN, [0, 0, 0])
}