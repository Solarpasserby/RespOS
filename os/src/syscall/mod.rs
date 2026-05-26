// os/src/syscall.rs

//! ### 系统调用模块

const SYSCALL_GETCWD: usize = 17;
const SYSCALL_DUP: usize = 23;
const SYSCALL_DUP3: usize = 24;
const SYSCALL_MKDIRAT: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_CHDIR: usize = 49;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_PIPE2: usize = 59;
const SYSCALL_GETDENTS64: usize = 61;
const SYSCALL_LSEEK: usize = 62;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_STAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_NANOSLEEP: usize = 101;
const SYSCALL_SCHED_YIELD: usize = 124;
const SYSCALL_KILL: usize = 129;
const SYSCALL_SIGACTION: usize = 134;
const SYSCALL_SIGPROCMASK: usize = 135;
const SYSCALL_SIGRETURN: usize = 139;
const SYSCALL_SETPRIORITY: usize = 140;
const SYSCALL_REBOOT: usize = 142;
const SYSCALL_TIMES: usize = 153;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GETTIMEOFDAY: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_BRK: usize = 214;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_CLONE: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_WAIT4: usize = 260;
// FIXME: 把系统调用号按大小排布

mod errno;
mod fs;
mod mm;
mod process;
mod system;
mod time;

pub use errno::*;
use fs::*;
use mm::*;
use process::*;
use system::*;
use time::*;

use crate::task::SignalAction;

pub fn syscall(syscall_id: usize, args: [usize; 6]) -> SysResult<usize> {
    match syscall_id {
        SYSCALL_GETCWD => sys_getcwd(args[0] as *mut u8, args[1]),
        SYSCALL_DUP => sys_dup(args[0]),
        SYSCALL_DUP3 => sys_dup3(args[0], args[1], args[2]),
        SYSCALL_MKDIRAT => sys_mkdirat(args[0] as isize, args[1] as *const u8, args[2]),
        SYSCALL_UNLINKAT => sys_unlinkat(args[0] as isize, args[1] as *const u8, args[2]),
        SYSCALL_LINKAT => sys_linkat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as isize,
            args[3] as *const u8,
            args[4],
        ),
        SYSCALL_UMOUNT2 => sys_umount2(args[0] as *const u8, args[1]),
        SYSCALL_MOUNT => sys_mount(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4] as *const u8,
        ),
        SYSCALL_CHDIR => sys_chdir(args[0] as *const u8),
        SYSCALL_OPENAT => sys_openat(args[0] as isize, args[1] as *const u8, args[2], args[3]),
        SYSCALL_CLOSE => sys_close(args[0]),
        SYSCALL_PIPE2 => sys_pipe2(args[0] as *mut [i32; 2], args[1]),
        SYSCALL_GETDENTS64 => sys_getdents64(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_LSEEK => sys_lseek(args[0], args[1] as isize, args[2]),
        SYSCALL_READ => sys_read(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_WRITE => sys_write(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_STAT => sys_stat(args[0] as *const u8, args[1] as *mut crate::fs::Stat),
        SYSCALL_FSTAT => sys_fstat(args[0], args[1] as *mut crate::fs::Stat),
        SYSCALL_EXIT => sys_exit(args[0] as i32),
        SYSCALL_NANOSLEEP => sys_nanosleep(args[0] as *const TimeVal, args[1] as *mut TimeVal),
        SYSCALL_SCHED_YIELD => sys_sched_yield(),
        SYSCALL_SETPRIORITY => sys_setpriority(args[0], args[1], args[2] as isize),
        SYSCALL_TIMES => sys_times(args[0] as *mut Tms),
        SYSCALL_UNAME => sys_uname(args[0] as *mut UtsName),
        SYSCALL_KILL => sys_kill(args[0], args[1] as i32),
        SYSCALL_SIGACTION => sys_sigaction(
            args[0] as i32,
            args[1] as *const SignalAction,
            args[2] as *mut SignalAction,
        ),
        SYSCALL_SIGPROCMASK => sys_sigprocmask(args[0] as u32),
        SYSCALL_SIGRETURN => sys_sigreturn(),
        SYSCALL_REBOOT => sys_reboot(),
        SYSCALL_GETTIMEOFDAY => sys_gettimeofday(args[0] as *mut TimeVal, args[1]),
        SYSCALL_GETPID => sys_getpid(),
        SYSCALL_GETPPID => sys_getppid(),
        SYSCALL_BRK => sys_brk(args[0]),
        SYSCALL_MUNMAP => sys_munmap(args[0], args[1]),
        SYSCALL_CLONE => sys_clone(args[0], args[1], args[2], args[3], args[4]),
        SYSCALL_EXECVE => sys_execve(
            args[0] as *const u8,
            args[1] as *const usize,
            args[2] as *const usize,
        ),
        SYSCALL_MMAP => sys_mmap(
            args[0],
            args[1],
            args[2],
            args[3],
            args[4] as isize,
            args[5],
        ),
        SYSCALL_WAIT4 => sys_wait4(
            args[0] as isize,
            args[1] as *mut i32,
            args[2],
            args[3] as *mut RUsage,
        ),
        _ => Err(Errno::ENOSYS),
    }
}
