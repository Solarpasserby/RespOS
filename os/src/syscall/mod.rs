// os/src/syscall.rs

//! ### 系统调用模块

const SYSCALL_SETXATTR: usize = 5;
const SYSCALL_LSETXATTR: usize = 6;
const SYSCALL_FSETXATTR: usize = 7;
const SYSCALL_GETXATTR: usize = 8;
const SYSCALL_LGETXATTR: usize = 9;
const SYSCALL_FGETXATTR: usize = 10;
const SYSCALL_LISTXATTR: usize = 11;
const SYSCALL_LLISTXATTR: usize = 12;
const SYSCALL_FLISTXATTR: usize = 13;
const SYSCALL_REMOVEXATTR: usize = 14;
const SYSCALL_LREMOVEXATTR: usize = 15;
const SYSCALL_FREMOVEXATTR: usize = 16;
const SYSCALL_GETCWD: usize = 17;
const SYSCALL_DUP: usize = 23;
const SYSCALL_DUP3: usize = 24;
const SYSCALL_FCNTL: usize = 25;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_EVENTFD2: usize = 19;
const SYSCALL_EPOLL_CREATE1: usize = 20;
const SYSCALL_INOTIFY_INIT1: usize = 26;
const SYSCALL_MKNODAT: usize = 33;
const SYSCALL_FLOCK: usize = 32;
const SYSCALL_MKDIRAT: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_SYMLINKAT: usize = 36;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_STATFS: usize = 43;
const SYSCALL_FSTATFS: usize = 44;
const SYSCALL_TRUNCATE: usize = 45;
const SYSCALL_FTRUNCATE: usize = 46;
const SYSCALL_FALLOCATE: usize = 47;
const SYSCALL_FACCESSAT: usize = 48;
const SYSCALL_CHDIR: usize = 49;
const SYSCALL_FCHDIR: usize = 50;
const SYSCALL_CHROOT: usize = 51;
const SYSCALL_FCHMOD: usize = 52;
const SYSCALL_FCHMODAT: usize = 53;
const SYSCALL_FCHOWNAT: usize = 54;
const SYSCALL_FCHOWN: usize = 55;
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
const SYSCALL_PWRITE64: usize = 68;
const SYSCALL_PREADV: usize = 69;
const SYSCALL_PWRITEV: usize = 70;
const SYSCALL_SENDFILE: usize = 71;
const SYSCALL_PSELECT6: usize = 72;
const SYSCALL_PPOLL: usize = 73;
const SYSCALL_SIGNALFD4: usize = 74;
const SYSCALL_VMSPLICE: usize = 75;
const SYSCALL_SPLICE: usize = 76;
const SYSCALL_TEE: usize = 77;
const SYSCALL_READLINKAT: usize = 78;
const SYSCALL_FSTATAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_FSYNC: usize = 82;
const SYSCALL_FDATASYNC: usize = 83;
const SYSCALL_SYNC_FILE_RANGE: usize = 84;
const SYSCALL_TIMERFD_CREATE: usize = 85;
const SYSCALL_TIMERFD_SETTIME: usize = 86;
const SYSCALL_TIMERFD_GETTIME: usize = 87;
const SYSCALL_UTIMENSAT: usize = 88;
const SYSCALL_CAPGET: usize = 90;
const SYSCALL_CAPSET: usize = 91;
const SYSCALL_PERSONALITY: usize = 92;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_EXIT_GROUP: usize = 94;
const SYSCALL_WAITID: usize = 95;
const SYSCALL_SET_TID_ADDRESS: usize = 96;
const SYSCALL_FUTEX: usize = 98;
const SYSCALL_SET_ROBUST_LIST: usize = 99;
const SYSCALL_GET_ROBUST_LIST: usize = 100;
const SYSCALL_NANOSLEEP: usize = 101;
const SYSCALL_GETITIMER: usize = 102;
const SYSCALL_SETITIMER: usize = 103;
const SYSCALL_TIMER_CREATE: usize = 107;
const SYSCALL_TIMER_GETTIME: usize = 108;
const SYSCALL_TIMER_GETOVERRUN: usize = 109;
const SYSCALL_TIMER_SETTIME: usize = 110;
const SYSCALL_TIMER_DELETE: usize = 111;
const SYSCALL_CLOCK_SETTIME: usize = 112;
const SYSCALL_CLOCK_GETTIME: usize = 113;
const SYSCALL_CLOCK_GETRES: usize = 114;
const SYSCALL_CLOCK_NANOSLEEP: usize = 115;
const SYSCALL_SYSLOG: usize = 116;
const SYSCALL_SCHED_SETPARAM: usize = 118;
const SYSCALL_SCHED_SETSCHEDULER: usize = 119;
const SYSCALL_SCHED_GETSCHEDULER: usize = 120;
const SYSCALL_SCHED_GETPARAM: usize = 121;
const SYSCALL_SCHED_SETAFFINITY: usize = 122;
const SYSCALL_SCHED_GETAFFINITY: usize = 123;
const SYSCALL_SCHED_YIELD: usize = 124;
const SYSCALL_SCHED_GET_PRIORITY_MAX: usize = 125;
const SYSCALL_SCHED_GET_PRIORITY_MIN: usize = 126;
const SYSCALL_SCHED_RR_GET_INTERVAL: usize = 127;
const SYSCALL_KILL: usize = 129;
const SYSCALL_TKILL: usize = 130;
const SYSCALL_TGKILL: usize = 131;
const SYSCALL_SIGALTSTACK: usize = 132;
const SYSCALL_RT_SIGSUSPEND: usize = 133;
const SYSCALL_SIGACTION: usize = 134;
const SYSCALL_SIGPROCMASK: usize = 135;
const SYSCALL_RT_SIGPENDING: usize = 136;
const SYSCALL_RT_SIGTIMEDWAIT: usize = 137;
const SYSCALL_RT_SIGQUEUEINFO: usize = 138;
pub const SYSCALL_SIGRETURN: usize = 139;
const SYSCALL_SETPRIORITY: usize = 140;
const SYSCALL_GETPRIORITY: usize = 141;
const SYSCALL_REBOOT: usize = 142;
const SYSCALL_SETREGID: usize = 143;
const SYSCALL_SETGID: usize = 144;
const SYSCALL_SETREUID: usize = 145;
const SYSCALL_SETUID: usize = 146;
const SYSCALL_SETRESUID: usize = 147;
const SYSCALL_GETRESUID: usize = 148;
const SYSCALL_SETRESGID: usize = 149;
const SYSCALL_GETRESGID: usize = 150;
const SYSCALL_SETFSUID: usize = 151;
const SYSCALL_SETFSGID: usize = 152;
const SYSCALL_TIMES: usize = 153;
const SYSCALL_SETPGID: usize = 154;
const SYSCALL_GETPGID: usize = 155;
const SYSCALL_SETSID: usize = 157;
const SYSCALL_GETGROUPS: usize = 158;
const SYSCALL_SETGROUPS: usize = 159;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_SETHOSTNAME: usize = 161;
const SYSCALL_SETDOMAINNAME: usize = 162;
const SYSCALL_GETRLIMIT: usize = 163;
const SYSCALL_SETRLIMIT: usize = 164;
const SYSCALL_GETRUSAGE: usize = 165;
const SYSCALL_UMASK: usize = 166;
const SYSCALL_GETTIMEOFDAY: usize = 169;
const SYSCALL_SETTIMEOFDAY: usize = 170;
const SYSCALL_ADJTIMEX: usize = 171;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_GETUID: usize = 174;
const SYSCALL_GETEUID: usize = 175;
const SYSCALL_GETGID: usize = 176;
const SYSCALL_GETEGID: usize = 177;
const SYSCALL_GETTID: usize = 178;
const SYSCALL_SYSINFO: usize = 179;
const SYSCALL_SHMGET: usize = 194;
const SYSCALL_SHMCTL: usize = 195;
const SYSCALL_SHMAT: usize = 196;
const SYSCALL_SHMDT: usize = 197;
const SYSCALL_SOCKET: usize = 198;
const SYSCALL_SOCKETPAIR: usize = 199;
const SYSCALL_BIND: usize = 200;
const SYSCALL_LISTEN: usize = 201;
const SYSCALL_ACCEPT: usize = 202;
const SYSCALL_CONNECT: usize = 203;
const SYSCALL_GETSOCKNAME: usize = 204;
const SYSCALL_GETPEERNAME: usize = 205;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;
const SYSCALL_SETSOCKOPT: usize = 208;
const SYSCALL_GETSOCKOPT: usize = 209;
const SYSCALL_SHUTDOWN: usize = 210;
const SYSCALL_SENDMSG: usize = 211;
const SYSCALL_RECVMSG: usize = 212;
const SYSCALL_BRK: usize = 214;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_CLONE: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_FADVISE64: usize = 223;
const SYSCALL_MPROTECT: usize = 226;
const SYSCALL_MSYNC: usize = 227;
const SYSCALL_MLOCK: usize = 228;
const SYSCALL_MUNLOCK: usize = 229;
const SYSCALL_MREMAP: usize = 216;
const SYSCALL_MADVISE: usize = 233;
const SYSCALL_GET_MEMPOLICY: usize = 236;
const SYSCALL_PERF_EVENT_OPEN: usize = 241;
const SYSCALL_ACCEPT4: usize = 242;
const SYSCALL_RECVMMSG: usize = 243;
const SYSCALL_COPY_FILE_RANGE: usize = 285;
const SYSCALL_PREADV2: usize = 286;
const SYSCALL_PWRITEV2: usize = 287;
const SYSCALL_WAIT4: usize = 260;
const SYSCALL_PRLIMIT64: usize = 261;
const SYSCALL_FANOTIFY_INIT: usize = 262;
const SYSCALL_CLOCK_ADJTIME: usize = 266;
const SYSCALL_SENDMMSG: usize = 269;
const SYSCALL_SCHED_SETATTR: usize = 274;
const SYSCALL_SCHED_GETATTR: usize = 275;
const SYSCALL_RENAMEAT2: usize = 276;
const SYSCALL_GETRANDOM: usize = 278;
const SYSCALL_MEMFD_CREATE: usize = 279;
const SYSCALL_BPF: usize = 280;
const SYSCALL_EXECVEAT: usize = 281;
const SYSCALL_USERFAULTFD: usize = 282;
const SYSCALL_STATX: usize = 291;
const SYSCALL_IO_URING_SETUP: usize = 425;
const SYSCALL_OPEN_TREE: usize = 428;
const SYSCALL_FSOPEN: usize = 430;
const SYSCALL_FSPICK: usize = 433;
const SYSCALL_PIDFD_OPEN: usize = 434;
const SYSCALL_CLOSE_RANGE: usize = 436;
const SYSCALL_OPENAT2: usize = 437;
const SYSCALL_FACCESSAT2: usize = 439;
const SYSCALL_MEMFD_SECRET: usize = 447;

mod errno;
mod fs;
pub(crate) mod ipc;
mod mm;
mod net;
mod process;
mod signal;
mod special_fd;
mod system;
mod time;

use crate::fs::Stat;
use crate::signal::LinuxSigInfo;
use crate::signal::SigSet;
use crate::signal::sig_stack::SignalStack;
use crate::timer::TimeSpec;
pub use errno::*;
use fs::*;
use ipc::*;
use mm::*;
use net::*;
use process::*;
use signal::*;
use special_fd::*;
use system::*;
use time::*;

pub use time::{
    check_nanosleep_timeouts, check_posix_timers, finish_task_timeout, register_task_timeout,
};

pub fn check_all_task_timers() {
    crate::task::check_futex_timeouts();
    check_nanosleep_timeouts();
    check_timerfd_expirations();
    crate::task::TASK_MANAGER.for_each(|task| {
        task.check_real_timer();
    });
    check_posix_timers();
}

fn merge_offset_arg(low: usize, high: usize) -> isize {
    (((high as u64) << 32) | ((low as u64) & 0xffff_ffff)) as i64 as isize
}

pub fn syscall(syscall_id: usize, args: [usize; 6]) -> SysResult<usize> {
    match syscall_id {
        SYSCALL_SETXATTR => sys_setxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4],
        ),
        SYSCALL_LSETXATTR => sys_lsetxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4],
        ),
        SYSCALL_FSETXATTR => sys_fsetxattr(
            args[0],
            args[1] as *const u8,
            args[2] as *const u8,
            args[3],
            args[4],
        ),
        SYSCALL_GETXATTR => sys_getxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3],
        ),
        SYSCALL_LGETXATTR => sys_lgetxattr(
            args[0] as *const u8,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3],
        ),
        SYSCALL_FGETXATTR => {
            sys_fgetxattr(args[0], args[1] as *const u8, args[2] as *mut u8, args[3])
        }
        SYSCALL_LISTXATTR => sys_listxattr(args[0] as *const u8, args[1] as *mut u8, args[2]),
        SYSCALL_LLISTXATTR => sys_llistxattr(args[0] as *const u8, args[1] as *mut u8, args[2]),
        SYSCALL_FLISTXATTR => sys_flistxattr(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_REMOVEXATTR => sys_removexattr(args[0] as *const u8, args[1] as *const u8),
        SYSCALL_LREMOVEXATTR => sys_lremovexattr(args[0] as *const u8, args[1] as *const u8),
        SYSCALL_FREMOVEXATTR => sys_fremovexattr(args[0], args[1] as *const u8),
        SYSCALL_GETCWD => sys_getcwd(args[0] as *mut u8, args[1]),
        SYSCALL_DUP => sys_dup(args[0]),
        SYSCALL_DUP3 => sys_dup3(args[0], args[1], args[2]),
        SYSCALL_EVENTFD2 => sys_eventfd2(args[0], args[1]),
        SYSCALL_EPOLL_CREATE1 => sys_epoll_create1(args[0]),
        SYSCALL_INOTIFY_INIT1 => sys_inotify_init1(args[0]),
        SYSCALL_FCNTL => sys_fcntl(args[0], args[1], args[2]),
        SYSCALL_IOCTL => sys_ioctl(args[0], args[1], args[2]),
        SYSCALL_FLOCK => sys_flock(args[0], args[1]),
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
        SYSCALL_FTRUNCATE => sys_ftruncate(args[0], args[1] as isize),
        SYSCALL_TRUNCATE => sys_truncate(args[0] as *const u8, args[1] as isize),
        SYSCALL_FALLOCATE => sys_fallocate(args[0], args[1], args[2] as isize, args[3] as isize),
        SYSCALL_FACCESSAT => sys_faccessat(args[0] as isize, args[1] as *const u8, args[2], 0),
        SYSCALL_FACCESSAT2 => {
            sys_faccessat(args[0] as isize, args[1] as *const u8, args[2], args[3])
        }
        SYSCALL_CHDIR => sys_chdir(args[0] as *const u8),
        SYSCALL_FCHDIR => sys_fchdir(args[0]),
        SYSCALL_CHROOT => sys_chroot(args[0] as *const u8),
        SYSCALL_MKNODAT => sys_mknodat(args[0] as isize, args[1] as *const u8, args[2], args[3]),
        SYSCALL_FCHMOD => sys_fchmod(args[0], args[1]),
        SYSCALL_FCHMODAT => sys_fchmodat(args[0] as isize, args[1] as *const u8, args[2]),
        SYSCALL_FCHOWNAT => sys_fchownat(
            args[0] as isize,
            args[1] as *const u8,
            args[2],
            args[3],
            args[4],
        ),
        SYSCALL_FCHOWN => sys_fchown(args[0], args[1], args[2]),
        SYSCALL_OPENAT => sys_openat(args[0] as isize, args[1] as *const u8, args[2], args[3]),
        SYSCALL_CLOSE => sys_close(args[0]),
        SYSCALL_PIPE2 => sys_pipe2(args[0] as *mut [i32; 2], args[1]),
        SYSCALL_GETDENTS64 => sys_getdents64(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_LSEEK => sys_lseek(args[0], args[1] as isize, args[2]),
        SYSCALL_READ => sys_read(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_WRITE => sys_write(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_READV => sys_readv(args[0], args[1] as *const IoVec, args[2]),
        SYSCALL_WRITEV => sys_writev(args[0], args[1] as *const IoVec, args[2]),
        SYSCALL_PREADV => sys_preadv(args[0], args[1] as *const IoVec, args[2], args[3] as isize),
        SYSCALL_PWRITEV => sys_pwritev(args[0], args[1] as *const IoVec, args[2], args[3] as isize),
        SYSCALL_SENDFILE => sys_sendfile(args[0], args[1], args[2] as *mut i64, args[3]),
        SYSCALL_PREAD64 => sys_pread64(args[0], args[1] as *mut u8, args[2], args[3] as isize),
        SYSCALL_PWRITE64 => sys_pwrite64(args[0], args[1] as *mut u8, args[2], args[3] as isize),
        SYSCALL_PSELECT6 => sys_pselect6(
            args[0],
            args[1],
            args[2],
            args[3],
            args[4] as *const TimeSpec,
            args[5],
        ),
        SYSCALL_PPOLL => sys_ppoll(
            args[0] as *mut PollFd,
            args[1],
            args[2] as *const TimeSpec,
            args[3] as *const SigSet,
            args[4],
        ),
        SYSCALL_SIGNALFD4 => {
            sys_signalfd4(args[0] as isize, args[1] as *const u8, args[2], args[3])
        }
        SYSCALL_VMSPLICE => sys_vmsplice(args[0], args[1] as *const IoVec, args[2], args[3]),
        SYSCALL_SPLICE => sys_splice(
            args[0],
            args[1] as *mut i64,
            args[2],
            args[3] as *mut i64,
            args[4],
            args[5],
        ),
        SYSCALL_TEE => sys_tee(args[0], args[1], args[2], args[3]),
        SYSCALL_READLINKAT => sys_readlinkat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut u8,
            args[3],
        ),
        SYSCALL_FSTATAT => sys_fstatat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *mut Stat,
            args[3],
        ),
        SYSCALL_FSTAT => sys_fstat(args[0], args[1] as *mut Stat),
        SYSCALL_FSYNC => sys_fsync(args[0]),
        SYSCALL_FDATASYNC => sys_fdatasync(args[0]),
        SYSCALL_SYNC_FILE_RANGE => sys_sync_file_range(
            args[0] as isize,
            args[1] as isize,
            args[2] as isize,
            args[3],
        ),
        SYSCALL_TIMERFD_CREATE => sys_timerfd_create(args[0], args[1]),
        SYSCALL_TIMERFD_SETTIME => sys_timerfd_settime(
            args[0],
            args[1],
            args[2] as *const ITimerSpec,
            args[3] as *mut ITimerSpec,
        ),
        SYSCALL_TIMERFD_GETTIME => sys_timerfd_gettime(args[0], args[1] as *mut ITimerSpec),
        SYSCALL_FSTATFS => sys_fstatfs(args[0], args[1] as *mut crate::fs::Statfs64),
        SYSCALL_UTIMENSAT => sys_utimensat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *const TimeSpec,
            args[3],
        ),
        SYSCALL_CAPGET => sys_capget(args[0] as *mut CapUserHeader, args[1] as *mut CapUserData),
        SYSCALL_CAPSET => sys_capset(
            args[0] as *const CapUserHeader,
            args[1] as *const CapUserData,
        ),
        SYSCALL_PERSONALITY => sys_personality(args[0]),
        SYSCALL_EXIT => sys_exit(args[0] as i32),
        SYSCALL_EXIT_GROUP => sys_exit_group(args[0] as i32),
        SYSCALL_WAITID => sys_waitid(
            args[0],
            args[1],
            args[2] as *mut LinuxSigInfo,
            args[3],
            args[4],
        ),
        SYSCALL_SET_TID_ADDRESS => sys_set_tid_address(args[0]),
        SYSCALL_FUTEX => sys_futex(
            args[0] as *const i32,
            args[1],
            args[2],
            args[3],
            args[4],
            args[5],
        ),
        SYSCALL_SET_ROBUST_LIST => sys_set_robust_list(args[0], args[1]),
        SYSCALL_GET_ROBUST_LIST => {
            sys_get_robust_list(args[0], args[1] as *mut usize, args[2] as *mut usize)
        }
        SYSCALL_NANOSLEEP => sys_nanosleep(args[0] as *const TimeSpec, args[1] as *mut TimeSpec),
        SYSCALL_GETITIMER => sys_getitimer(args[0], args[1] as *mut ITimerVal),
        SYSCALL_SETITIMER => sys_setitimer(
            args[0],
            args[1] as *const ITimerVal,
            args[2] as *mut ITimerVal,
        ),
        SYSCALL_TIMER_CREATE => {
            sys_timer_create(args[0], args[1] as *const SigEvent, args[2] as *mut i32)
        }
        SYSCALL_TIMER_GETTIME => sys_timer_gettime(args[0], args[1] as *mut ITimerSpec),
        SYSCALL_TIMER_GETOVERRUN => sys_timer_getoverrun(args[0]),
        SYSCALL_TIMER_SETTIME => sys_timer_settime(
            args[0],
            args[1],
            args[2] as *const ITimerSpec,
            args[3] as *mut ITimerSpec,
        ),
        SYSCALL_TIMER_DELETE => sys_timer_delete(args[0]),
        SYSCALL_CLOCK_SETTIME => sys_clock_settime(args[0], args[1] as *const TimeSpec),
        SYSCALL_CLOCK_GETTIME => sys_clock_gettime(args[0], args[1] as *mut TimeSpec),
        SYSCALL_CLOCK_GETRES => sys_clock_getres(args[0], args[1] as *mut TimeSpec),
        SYSCALL_CLOCK_NANOSLEEP => sys_clock_nanosleep(
            args[0],
            args[1],
            args[2] as *const TimeSpec,
            args[3] as *mut TimeSpec,
        ),
        SYSCALL_SYSLOG => sys_syslog(args[0], args[1] as *mut u8, args[2] as isize),
        SYSCALL_SCHED_SETPARAM => sys_sched_setparam(args[0] as isize, args[1] as *const i32),
        SYSCALL_SCHED_SETSCHEDULER => {
            sys_sched_setscheduler(args[0] as isize, args[1], args[2] as *const i32)
        }
        SYSCALL_SCHED_GETSCHEDULER => sys_sched_getscheduler(args[0] as isize),
        SYSCALL_SCHED_GETPARAM => sys_sched_getparam(args[0] as isize, args[1] as *mut i32),
        SYSCALL_SCHED_SETAFFINITY => {
            sys_sched_setaffinity(args[0] as isize, args[1], args[2] as *const u8)
        }
        SYSCALL_SCHED_GETAFFINITY => {
            sys_sched_getaffinity(args[0] as isize, args[1], args[2] as *mut u8)
        }
        SYSCALL_SCHED_YIELD => sys_sched_yield(),
        SYSCALL_SCHED_GET_PRIORITY_MAX => sys_sched_get_priority_max(args[0] as isize),
        SYSCALL_SCHED_GET_PRIORITY_MIN => sys_sched_get_priority_min(args[0] as isize),
        SYSCALL_SCHED_RR_GET_INTERVAL => {
            sys_sched_rr_get_interval(args[0] as isize, args[1] as *mut TimeSpec)
        }
        SYSCALL_KILL => sys_kill(args[0], args[1] as i32),
        SYSCALL_TKILL => sys_tkill(args[0], args[1] as i32),
        SYSCALL_TGKILL => sys_tgkill(args[0], args[1], args[2] as i32),
        SYSCALL_SIGALTSTACK => {
            sys_sigaltstack(args[0] as *const SignalStack, args[1] as *mut SignalStack)
        }
        SYSCALL_RT_SIGSUSPEND => sys_rt_sigsuspend(args[0] as *const SigSet, args[1]),
        SYSCALL_SIGACTION => {
            sys_sigaction(args[0] as i32, args[1] as *const u8, args[2] as *mut u8)
        }
        SYSCALL_SIGPROCMASK => sys_sigprocmask(args[0], args[1], args[2], args[3]),
        SYSCALL_RT_SIGPENDING => sys_rt_sigpending(args[0] as *mut SigSet, args[1]),
        SYSCALL_RT_SIGTIMEDWAIT => sys_rt_sigtimedwait(args[0], args[1], args[2], args[3]),
        SYSCALL_RT_SIGQUEUEINFO => {
            sys_rt_sigqueueinfo(args[0], args[1] as i32, args[2] as *const LinuxSigInfo)
        }
        SYSCALL_SIGRETURN => sys_sigreturn(),
        SYSCALL_SETPRIORITY => sys_setpriority(args[0], args[1], args[2] as isize),
        SYSCALL_GETPRIORITY => sys_getpriority(args[0], args[1]),
        SYSCALL_REBOOT => sys_reboot(),
        SYSCALL_SETREGID => sys_setregid(args[0], args[1]),
        SYSCALL_SETGID => sys_setgid(args[0]),
        SYSCALL_SETREUID => sys_setreuid(args[0], args[1]),
        SYSCALL_SETUID => sys_setuid(args[0]),
        SYSCALL_SETRESUID => sys_setresuid(args[0], args[1], args[2]),
        SYSCALL_GETRESUID => sys_getresuid(
            args[0] as *mut u32,
            args[1] as *mut u32,
            args[2] as *mut u32,
        ),
        SYSCALL_SETRESGID => sys_setresgid(args[0], args[1], args[2]),
        SYSCALL_GETRESGID => sys_getresgid(
            args[0] as *mut u32,
            args[1] as *mut u32,
            args[2] as *mut u32,
        ),
        SYSCALL_SETFSUID => sys_setfsuid(args[0]),
        SYSCALL_SETFSGID => sys_setfsgid(args[0]),
        SYSCALL_TIMES => sys_times(args[0] as *mut Tms),
        SYSCALL_SETPGID => sys_setpgid(args[0], args[1]),
        SYSCALL_GETPGID => sys_getpgid(args[0]),
        SYSCALL_SETSID => sys_setsid(),
        SYSCALL_GETGROUPS => sys_getgroups(args[0], args[1] as *mut u32),
        SYSCALL_SETGROUPS => sys_setgroups(args[0], args[1] as *const u32),
        SYSCALL_UNAME => sys_uname(args[0] as *mut UtsName),
        SYSCALL_SETHOSTNAME => sys_sethostname(args[0] as *const u8, args[1]),
        SYSCALL_SETDOMAINNAME => sys_setdomainname(args[0] as *const u8, args[1]),
        SYSCALL_GETRLIMIT => sys_getrlimit(args[0], args[1] as *mut RLimit),
        SYSCALL_SETRLIMIT => sys_setrlimit(args[0], args[1] as *const RLimit),
        SYSCALL_GETRUSAGE => sys_getrusage(args[0] as isize, args[1] as *mut RUsage),
        SYSCALL_UMASK => sys_umask(args[0]),
        SYSCALL_GETTIMEOFDAY => sys_gettimeofday(args[0] as *mut TimeVal, args[1] as *mut TimeZone),
        SYSCALL_SETTIMEOFDAY => {
            sys_settimeofday(args[0] as *const TimeVal, args[1] as *const TimeZone)
        }
        SYSCALL_ADJTIMEX => sys_adjtimex(args[0] as *mut Timex),
        SYSCALL_GETPID => sys_getpid(),
        SYSCALL_GETPPID => sys_getppid(),
        SYSCALL_GETUID => sys_getuid(),
        SYSCALL_GETEUID => sys_geteuid(),
        SYSCALL_GETGID => sys_getgid(),
        SYSCALL_GETEGID => sys_getegid(),
        SYSCALL_GETTID => sys_gettid(),
        SYSCALL_SYSINFO => sys_sysinfo(args[0] as *mut SysInfo),
        SYSCALL_SHMGET => sys_shmget(args[0] as isize, args[1], args[2]),
        SYSCALL_SHMCTL => sys_shmctl(args[0], args[1], args[2]),
        SYSCALL_SHMAT => sys_shmat(args[0], args[1], args[2]),
        SYSCALL_SHMDT => sys_shmdt(args[0]),
        SYSCALL_SOCKET => sys_socket(args[0], args[1], args[2]),
        SYSCALL_SOCKETPAIR => sys_socketpair(args[0], args[1], args[2], args[3] as *mut i32),
        SYSCALL_BIND => sys_bind(args[0], args[1], args[2]),
        SYSCALL_LISTEN => sys_listen(args[0], args[1]),
        SYSCALL_ACCEPT => sys_accept(args[0], args[1], args[2]),
        SYSCALL_CONNECT => sys_connect(args[0], args[1], args[2]),
        SYSCALL_GETSOCKNAME => sys_getsockname(args[0], args[1], args[2]),
        SYSCALL_GETPEERNAME => sys_getpeername(args[0], args[1], args[2]),
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
        SYSCALL_GETSOCKOPT => sys_getsockopt(args[0], args[1], args[2], args[3], args[4]),
        SYSCALL_SHUTDOWN => sys_shutdown(args[0], args[1]),
        SYSCALL_SENDMSG => sys_sendmsg(args[0], args[1], args[2]),
        SYSCALL_RECVMSG => sys_recvmsg(args[0], args[1], args[2]),
        SYSCALL_BRK => sys_brk(args[0]),
        SYSCALL_MUNMAP => sys_munmap(args[0], args[1]),
        SYSCALL_CLONE => sys_clone(args[0], args[1], args[2], args[3], args[4]),
        SYSCALL_ACCEPT4 => sys_accept4(args[0], args[1], args[2], args[3]),
        SYSCALL_RECVMMSG => sys_recvmmsg(args[0], args[1], args[2], args[3], args[4]),
        SYSCALL_COPY_FILE_RANGE => sys_copy_file_range(
            args[0],
            args[1] as *mut i64,
            args[2],
            args[3] as *mut i64,
            args[4],
            args[5],
        ),
        SYSCALL_EXECVE => sys_execve(
            args[0] as *const u8,
            args[1] as *const usize,
            args[2] as *const usize,
        ),
        SYSCALL_EXECVEAT => sys_execveat(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *const usize,
            args[3] as *const usize,
            args[4],
        ),
        SYSCALL_MMAP => sys_mmap(
            args[0],
            args[1],
            args[2],
            args[3],
            args[4] as isize,
            args[5],
        ),
        SYSCALL_FADVISE64 => sys_fadvise64(args[0], args[1] as isize, args[2] as isize, args[3]),
        SYSCALL_MPROTECT => sys_mprotect(args[0], args[1], args[2] as u32),
        SYSCALL_MREMAP => sys_mremap(args[0], args[1], args[2], args[3], args[4]),
        SYSCALL_MSYNC => sys_msync(args[0], args[1], args[2] as i32),
        SYSCALL_MLOCK => sys_mlock(args[0], args[1]),
        SYSCALL_MUNLOCK => sys_munlock(args[0], args[1]),
        SYSCALL_PERF_EVENT_OPEN => sys_perf_event_open(
            args[0] as *const u8,
            args[1] as isize,
            args[2] as isize,
            args[3] as isize,
            args[4],
        ),
        SYSCALL_PREADV2 => sys_preadv2(
            args[0],
            args[1] as *const IoVec,
            args[2],
            merge_offset_arg(args[3], args[4]),
            args[5] as i32,
        ),
        SYSCALL_PWRITEV2 => sys_pwritev2(
            args[0],
            args[1] as *const IoVec,
            args[2],
            merge_offset_arg(args[3], args[4]),
            args[5] as i32,
        ),
        SYSCALL_MADVISE => sys_madvise(args[0], args[1], args[2] as i32),
        SYSCALL_GET_MEMPOLICY => sys_get_mempolicy(
            args[0] as *mut i32,
            args[1] as *mut usize,
            args[2],
            args[3],
            args[4],
        ),
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
        SYSCALL_FANOTIFY_INIT => sys_fanotify_init(args[0], args[1]),
        SYSCALL_CLOCK_ADJTIME => sys_clock_adjtime(args[0], args[1] as *mut Timex),
        SYSCALL_SENDMMSG => sys_sendmmsg(args[0], args[1], args[2], args[3]),
        SYSCALL_SCHED_SETATTR => sys_sched_setattr(
            args[0] as isize,
            args[1] as *const SchedAttr,
            args[2] as u32,
        ),
        SYSCALL_SCHED_GETATTR => sys_sched_getattr(
            args[0] as isize,
            args[1] as *mut SchedAttr,
            args[2] as u32,
            args[3] as u32,
        ),
        SYSCALL_RENAMEAT2 => sys_renameat2(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as isize,
            args[3] as *const u8,
            args[4],
        ),
        SYSCALL_GETRANDOM => sys_getrandom(args[0] as *mut u8, args[1], args[2]),
        SYSCALL_MEMFD_CREATE => sys_memfd_create(args[0] as *const u8, args[1]),
        SYSCALL_BPF => sys_bpf(args[0], args[1] as *const u8, args[2]),
        SYSCALL_USERFAULTFD => sys_userfaultfd(args[0]),
        SYSCALL_IO_URING_SETUP => sys_io_uring_setup(args[0], args[1] as *const u8),
        SYSCALL_OPEN_TREE => sys_open_tree(args[0] as isize, args[1] as *const u8, args[2]),
        SYSCALL_FSOPEN => sys_fsopen(args[0] as *const u8, args[1]),
        SYSCALL_FSPICK => sys_fspick(args[0] as isize, args[1] as *const u8, args[2]),
        SYSCALL_PIDFD_OPEN => sys_pidfd_open(args[0], args[1]),
        SYSCALL_CLOSE_RANGE => sys_close_range(args[0], args[1], args[2]),
        SYSCALL_STATX => sys_statx(
            args[0] as isize,
            args[1] as *const u8,
            args[2],
            args[3] as u32,
            args[4] as *mut Statx,
        ),
        SYSCALL_OPENAT2 => sys_openat2(
            args[0] as isize,
            args[1] as *const u8,
            args[2] as *const OpenHow,
            args[3],
        ),
        SYSCALL_MEMFD_SECRET => sys_memfd_secret(args[0]),
        _ => Err(Errno::ENOSYS),
    }
}
