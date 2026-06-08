// os/src/syscall.rs

//! ### 系统调用模块

const SYSCALL_GETCWD: usize = 17;
const SYSCALL_DUP: usize = 23;
const SYSCALL_DUP3: usize = 24;
const SYSCALL_FCNTL: usize = 25;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_MKDIRAT: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_SYMLINKAT: usize = 36;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_STATFS: usize = 43;
const SYSCALL_FSTATFS: usize = 44;
const SYSCALL_FACCESSAT: usize = 48;
const SYSCALL_CHDIR: usize = 49;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_PIPE2: usize = 59;
const SYSCALL_GETDENTS64: usize = 61;
const SYSCALL_LSEEK: usize = 62;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_READV: usize = 65;
const SYSCALL_WRITEV: usize = 66;
const SYSCALL_PREAD64: usize = 67;
const SYSCALL_FSTATAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_UTIMENSAT: usize = 88;
const SYSCALL_READLINKAT: usize = 78;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_EXIT_GROUP: usize = 94;
const SYSCALL_SET_TID_ADDRESS: usize = 96;
const SYSCALL_FUTEX: usize = 98;
const SYSCALL_SET_ROBUST_LIST: usize = 99;
const SYSCALL_GET_ROBUST_LIST: usize = 100;
const SYSCALL_NANOSLEEP: usize = 101;
const SYSCALL_CLOCK_GETTIME: usize = 113;
const SYSCALL_SYSLOG: usize = 116;
const SYSCALL_SCHED_YIELD: usize = 124;
const SYSCALL_KILL: usize = 129;
const SYSCALL_TKILL: usize = 130;
const SYSCALL_TGKILL: usize = 131;
const SYSCALL_SIGACTION: usize = 134;
const SYSCALL_SIGPROCMASK: usize = 135;
const SYSCALL_RT_SIGTIMEDWAIT: usize = 137;
const SYSCALL_SIGRETURN: usize = 139;
const SYSCALL_SETPRIORITY: usize = 140;
const SYSCALL_REBOOT: usize = 142;
const SYSCALL_TIMES: usize = 153;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GETTIMEOFDAY: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_GETUID: usize = 174;
const SYSCALL_GETEUID: usize = 175;
const SYSCALL_GETGID: usize = 176;
const SYSCALL_GETEGID: usize = 177;
const SYSCALL_GETTID: usize = 178;
const SYSCALL_SYSINFO: usize = 179;
const SYSCALL_SOCKET: usize = 198;
const SYSCALL_BIND: usize = 200;
const SYSCALL_LISTEN: usize = 201;
const SYSCALL_ACCEPT: usize = 202;
const SYSCALL_CONNECT: usize = 203;
const SYSCALL_GETSOCKNAME: usize = 204;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;
const SYSCALL_SETSOCKOPT: usize = 208;
const SYSCALL_BRK: usize = 214;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_CLONE: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_MPROTECT: usize = 226;
const SYSCALL_MADVISE: usize = 233;
const SYSCALL_WAIT4: usize = 260;
const SYSCALL_PRLIMIT64: usize = 261;
const SYSCALL_RENAMEAT2: usize = 276;
const SYSCALL_GETRANDOM: usize = 278;

mod errno;
mod fs;
mod mm;
mod net;
mod process;
mod signal;
mod system;
mod time;

use crate::fs::Stat;
use crate::timer::TimeSpec;
pub use errno::*;
use fs::*;
use mm::*;
use net::*;
use process::*;
use signal::*;
use system::*;
use time::*;

pub fn syscall(syscall_id: usize, args: [usize; 6]) -> SysResult<usize> {
    match syscall_id {
        SYSCALL_GETCWD => sys_getcwd(args[0] as *mut u8, args[1]),
        SYSCALL_DUP => sys_dup(args[0]),
        SYSCALL_DUP3 => sys_dup3(args[0], args[1], args[2]),
        SYSCALL_FCNTL => sys_fcntl(args[0], args[1], args[2]),
        SYSCALL_IOCTL => sys_ioctl(args[0], args[1], args[2]),
        SYSCALL_MKDIRAT => sys_mkdirat(args[0] as isize, args[1] as *const u8, args[2]),
        SYSCALL_UNLINKAT => sys_unlinkat(args[0] as isize, args[1] as *const u8, args[2]),
        SYSCALL_SYMLINKAT => {
            sys_symlinkat(args[0] as *const u8, args[1] as isize, args[2] as *const u8)
        }
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
        SYSCALL_STATFS => sys_statfs(args[0] as *const u8, args[1] as *mut crate::fs::Statfs64),
        SYSCALL_FACCESSAT => {
            sys_faccessat(args[0] as isize, args[1] as *const u8, args[2], args[3])
        }
        SYSCALL_CHDIR => sys_chdir(args[0] as *const u8),
        SYSCALL_OPENAT => sys_openat(args[0] as isize, args[1] as *const u8, args[2], args[3]),
        SYSCALL_CLOSE => sys_close(args[0]),
        SYSCALL_PIPE2 => sys_pipe2(args[0] as *mut [i32; 2], args[1]),
        SYSCALL_GETDENTS64 => sys_getdents64(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_LSEEK => sys_lseek(args[0], args[1] as isize, args[2]),
        SYSCALL_READ => sys_read(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_WRITE => sys_write(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_READV => sys_readv(args[0], args[1] as *const IoVec, args[2]),
        SYSCALL_WRITEV => sys_writev(args[0], args[1] as *const IoVec, args[2]),
        SYSCALL_PREAD64 => sys_pread64(args[0], args[1] as *mut u8, args[2], args[3] as isize),
        SYSCALL_FSTATAT => sys_fstatat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut Stat,
            args[3],
        ),
        SYSCALL_FSTAT => sys_fstat(args[0], args[1] as *mut Stat),
        SYSCALL_FSTATFS => sys_fstatfs(args[0], args[1] as *mut crate::fs::Statfs64),
        SYSCALL_READLINKAT => sys_readlinkat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3],
        ),
        SYSCALL_UTIMENSAT => sys_utimensat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *const TimeSpec,
            args[3],
        ),
        SYSCALL_EXIT => sys_exit(args[0] as i32),
        SYSCALL_EXIT_GROUP => sys_exit_group(args[0] as i32),
        SYSCALL_SET_TID_ADDRESS => sys_set_tid_address(args[0]),
        SYSCALL_SET_ROBUST_LIST => sys_set_robust_list(args[0], args[1]),
        SYSCALL_GET_ROBUST_LIST => {
            sys_get_robust_list(args[0], args[1] as *mut usize, args[2] as *mut usize)
        }
        SYSCALL_FUTEX => sys_futex(
            args[0] as *const i32,
            args[1],
            args[2],
            args[3],
            args[4],
            args[5],
        ),
        SYSCALL_NANOSLEEP => sys_nanosleep(args[0] as *const TimeVal, args[1] as *mut TimeVal),
        SYSCALL_CLOCK_GETTIME => sys_clock_gettime(args[0], args[1] as *mut TimeSpec),
        SYSCALL_SYSLOG => sys_syslog(args[0], args[1] as *mut u8, args[2] as isize),
        SYSCALL_SCHED_YIELD => sys_sched_yield(),
        SYSCALL_SETPRIORITY => sys_setpriority(args[0], args[1], args[2] as isize),
        SYSCALL_TIMES => sys_times(args[0] as *mut Tms),
        SYSCALL_UNAME => sys_uname(args[0] as *mut UtsName),
        SYSCALL_KILL => sys_kill(args[0], args[1] as i32),
        SYSCALL_TKILL => sys_tkill(args[0], args[1] as i32),
        SYSCALL_TGKILL => sys_tgkill(args[0], args[1], args[2] as i32),
        SYSCALL_SIGACTION => {
            sys_sigaction(args[0] as i32, args[1] as *const u8, args[2] as *mut u8)
        }
        SYSCALL_SIGPROCMASK => sys_sigprocmask(args[0], args[1], args[2], args[3]),
        SYSCALL_SIGRETURN => sys_sigreturn(),
        SYSCALL_RT_SIGTIMEDWAIT => sys_rt_sigtimedwait(
            args[0], // set: 信号集指针
            args[1], // info: 信号信息输出指针
            args[2], // timeout_ptr: 超时时间指针
            args[3], // sigsetsize: 信号集大小
        ),
        SYSCALL_REBOOT => sys_reboot(),
        SYSCALL_GETTIMEOFDAY => sys_gettimeofday(args[0] as *mut TimeVal, args[1]),
        SYSCALL_GETPID => sys_getpid(),
        SYSCALL_GETPPID => sys_getppid(),
        SYSCALL_GETUID => sys_getuid(),
        SYSCALL_GETEUID => sys_geteuid(),
        SYSCALL_GETGID => sys_getgid(),
        SYSCALL_GETEGID => sys_getegid(),
        SYSCALL_GETTID => sys_gettid(),
        SYSCALL_SYSINFO => sys_sysinfo(args[0] as *mut SysInfo),
        SYSCALL_SOCKET => sys_socket(args[0], args[1], args[2]),
        SYSCALL_BIND => sys_bind(args[0], args[1], args[2]),
        SYSCALL_LISTEN => sys_listen(args[0], args[1]),
        SYSCALL_ACCEPT => sys_accept(args[0], args[1], args[2]),
        SYSCALL_CONNECT => sys_connect(args[0], args[1], args[2]),
        SYSCALL_GETSOCKNAME => sys_getsockname(args[0], args[1], args[2]),
        SYSCALL_SENDTO => sys_sendto(
            args[0],
            args[1] as *const u8,
            args[2],
            args[3],
            args[4],
            args[5],
        ),
        SYSCALL_RECVFROM => sys_recvfrom(
            args[0],
            args[1] as *mut u8,
            args[2],
            args[3],
            args[4],
            args[5],
        ),
        SYSCALL_SETSOCKOPT => sys_setsockopt(args[0], args[1], args[2], args[3], args[4]),
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
        SYSCALL_MPROTECT => sys_mprotect(args[0], args[1], args[2] as u32),
        SYSCALL_MADVISE => sys_madvise(args[0], args[1], args[2] as i32),
        SYSCALL_WAIT4 => sys_wait4(
            args[0] as isize,
            args[1] as *mut i32,
            args[2],
            args[3] as *mut RUsage,
        ),
        SYSCALL_PRLIMIT64 => sys_prlimit64(
            args[0],
            args[1],
            args[2] as *const RLimit,
            args[3] as *mut RLimit,
        ),
        SYSCALL_RENAMEAT2 => sys_renameat2(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as isize,
            args[3] as *const u8,
            args[4],
        ),
        SYSCALL_GETRANDOM => sys_getrandom(args[0] as *mut u8, args[1], args[2]),
        _ => Err(Errno::ENOSYS),
    }
}
