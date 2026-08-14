use crate::SignalAction;
use core::arch::asm;

const SYSCALL_SETXATTR: usize = 5;
const SYSCALL_FSETXATTR: usize = 7;
const SYSCALL_GETXATTR: usize = 8;
const SYSCALL_FGETXATTR: usize = 10;
const SYSCALL_LISTXATTR: usize = 11;
const SYSCALL_REMOVEXATTR: usize = 14;
const SYSCALL_GETCWD: usize = 17;
const SYSCALL_EPOLL_CREATE1: usize = 20;
const SYSCALL_EPOLL_CTL: usize = 21;
const SYSCALL_EPOLL_PWAIT: usize = 22;
const SYSCALL_DUP: usize = 23;
const SYSCALL_DUP3: usize = 24;
const SYSCALL_FCNTL: usize = 25;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_MKNODAT: usize = 33;
const SYSCALL_MKDIRAT: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_SYMLINKAT: usize = 36;
const SYSCALL_LINKAT: usize = 37;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_MOUNT: usize = 40;
const SYSCALL_TRUNCATE: usize = 45;
const SYSCALL_FTRUNCATE: usize = 46;
const SYSCALL_FALLOCATE: usize = 47;
const SYSCALL_CHDIR: usize = 49;
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
const SYSCALL_PPOLL: usize = 73;
const SYSCALL_SPLICE: usize = 76;
const SYSCALL_PREAD64: usize = 67;
const SYSCALL_PWRITE64: usize = 68;
const SYSCALL_PREADV: usize = 69;
const SYSCALL_PWRITEV: usize = 70;
const SYSCALL_READLINKAT: usize = 78;
const SYSCALL_UTIMENSAT: usize = 88;
const SYSCALL_FSTATAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_SYNC: usize = 81;
const SYSCALL_FSYNC: usize = 82;
const SYSCALL_FDATASYNC: usize = 83;
const SYSCALL_SYNC_FILE_RANGE: usize = 84;
const SYSCALL_SYNCFS: usize = 267;
const SYSCALL_TIMERFD_CREATE: usize = 85;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_EXIT_GROUP: usize = 94;
const SYSCALL_FUTEX: usize = 98;
const SYSCALL_NANOSLEEP: usize = 101;
const SYSCALL_TIMER_CREATE: usize = 107;
const SYSCALL_TIMER_GETTIME: usize = 108;
const SYSCALL_TIMER_SETTIME: usize = 110;
const SYSCALL_TIMER_DELETE: usize = 111;
const SYSCALL_CLOCK_SETTIME: usize = 112;
const SYSCALL_CLOCK_GETTIME: usize = 113;
const SYSCALL_CLOCK_GETRES: usize = 114;
const SYSCALL_SCHED_SETAFFINITY: usize = 122;
const SYSCALL_SCHED_GETAFFINITY: usize = 123;
const SYSCALL_SCHED_YIELD: usize = 124;
const SYSCALL_SETPRIORITY: usize = 140;
const SYSCALL_TIMES: usize = 153;
const SYSCALL_SETPGID: usize = 154;
const SYSCALL_GETPGID: usize = 155;
const SYSCALL_GETSID: usize = 156;
const SYSCALL_SETSID: usize = 157;
const SYSCALL_SHMGET: usize = 194;
const SYSCALL_SHMCTL: usize = 195;
const SYSCALL_SHMAT: usize = 196;
const SYSCALL_SHMDT: usize = 197;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GETRUSAGE: usize = 165;
const SYSCALL_KILL: usize = 129;
const SYSCALL_SIGACTION: usize = 134;
const SYSCALL_SIGPROCMASK: usize = 135;
const SYSCALL_SIGPENDING: usize = 136;
const SYSCALL_SIGQUEUEINFO: usize = 138;
const SYSCALL_SIGRETURN: usize = 139;
const SYSCALL_REBOOT: usize = 142;
const SYSCALL_GETTIMEOFDAY: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_GETTID: usize = 178;
const SYSCALL_GETUID: usize = 174;
const SYSCALL_GETGID: usize = 176;
const SYSCALL_SETFSUID: usize = 151;
const SYSCALL_SETFSGID: usize = 152;
const SYSCALL_GETGROUPS: usize = 158;
const SYSCALL_SETGROUPS: usize = 159;
const SYSCALL_UMASK: usize = 166;
const SYSCALL_FACCESSAT2: usize = 439;
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
const SYSCALL_BRK: usize = 214;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_CLONE: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_MSYNC: usize = 227;
const SYSCALL_MPROTECT: usize = 226;
const SYSCALL_MEMFD_CREATE: usize = 279;
const SYSCALL_RENAMEAT2: usize = 276;
const SYSCALL_STATX: usize = 291;
const SYSCALL_WAIT4: usize = 260;
const SYSCALL_PRLIMIT64: usize = 261;
const SYSCALL_COPY_FILE_RANGE: usize = 285;
const AT_REMOVEDIR: usize = 0x200;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
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
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct ITimerSpec {
    pub interval: TimeSpec,
    pub value: TimeSpec,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct RLimit {
    pub cur: usize,
    pub max: usize,
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

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct StatxTimestamp {
    pub sec: i64,
    pub nsec: u32,
    pub _pad: i32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Statx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub _spare0: [u16; 1],
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: StatxTimestamp,
    pub stx_btime: StatxTimestamp,
    pub stx_ctime: StatxTimestamp,
    pub stx_mtime: StatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub _spare3: [u64; 12],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoVec {
    pub base: *mut u8,
    pub len: usize,
}

fn syscall(id: usize, args: [usize; 6]) -> isize {
    let mut ret: isize;
    unsafe {
        #[cfg(target_arch = "riscv64")]
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
        #[cfg(target_arch = "loongarch64")]
        asm!(
            "syscall 0",
            inlateout("$r4") args[0] => ret,
            in("$r5") args[1],
            in("$r6") args[2],
            in("$r7") args[3],
            in("$r8") args[4],
            in("$r9") args[5],
            in("$r11") id
        );
    }
    ret
}

pub fn sys_read(fd: usize, buf: &mut [u8]) -> isize {
    syscall(
        SYSCALL_READ,
        [fd, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0],
    )
}

pub fn sys_write(fd: usize, buf: &[u8]) -> isize {
    syscall(
        SYSCALL_WRITE,
        [fd, buf.as_ptr() as usize, buf.len(), 0, 0, 0],
    )
}

pub fn sys_splice(
    fd_in: usize,
    off_in: usize,
    fd_out: usize,
    off_out: usize,
    len: usize,
    flags: usize,
) -> isize {
    syscall(SYSCALL_SPLICE, [fd_in, off_in, fd_out, off_out, len, flags])
}

pub fn sys_pread64(fd: usize, buf: &mut [u8], offset: isize) -> isize {
    syscall(
        SYSCALL_PREAD64,
        [
            fd,
            buf.as_mut_ptr() as usize,
            buf.len(),
            offset as usize,
            0,
            0,
        ],
    )
}

pub fn sys_pwrite64(fd: usize, buf: &[u8], offset: isize) -> isize {
    syscall(
        SYSCALL_PWRITE64,
        [fd, buf.as_ptr() as usize, buf.len(), offset as usize, 0, 0],
    )
}

pub fn sys_preadv(fd: usize, iov: &[IoVec], offset: isize) -> isize {
    syscall(
        SYSCALL_PREADV,
        [fd, iov.as_ptr() as usize, iov.len(), offset as usize, 0, 0],
    )
}

pub fn sys_pwritev(fd: usize, iov: &[IoVec], offset: isize) -> isize {
    syscall(
        SYSCALL_PWRITEV,
        [fd, iov.as_ptr() as usize, iov.len(), offset as usize, 0, 0],
    )
}

pub fn sys_copy_file_range(fd_in: usize, fd_out: usize, len: usize) -> isize {
    syscall(SYSCALL_COPY_FILE_RANGE, [fd_in, 0, fd_out, 0, len, 0])
}

pub fn sys_getcwd(buf: &mut [u8]) -> isize {
    syscall(
        SYSCALL_GETCWD,
        [buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0, 0],
    )
}

pub fn sys_dup(fd: usize) -> isize {
    syscall(SYSCALL_DUP, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_dup3(fd_src: usize, fd_dst: usize, flags: usize) -> isize {
    syscall(SYSCALL_DUP3, [fd_src, fd_dst, flags, 0, 0, 0])
}

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    syscall(SYSCALL_FCNTL, [fd, cmd, arg, 0, 0, 0])
}

pub fn sys_ioctl(fd: usize, request: usize, arg: usize) -> isize {
    syscall(SYSCALL_IOCTL, [fd, request, arg, 0, 0, 0])
}

pub fn sys_epoll_create1(flags: usize) -> isize {
    syscall(SYSCALL_EPOLL_CREATE1, [flags, 0, 0, 0, 0, 0])
}

pub fn sys_epoll_ctl(epfd: usize, op: usize, fd: usize, event: *const u8) -> isize {
    syscall(SYSCALL_EPOLL_CTL, [epfd, op, fd, event as usize, 0, 0])
}

pub fn sys_epoll_pwait(
    epfd: usize,
    events: *mut u8,
    maxevents: usize,
    timeout_ms: isize,
    sigmask: *const u8,
    sigsetsize: usize,
) -> isize {
    syscall(
        SYSCALL_EPOLL_PWAIT,
        [
            epfd,
            events as usize,
            maxevents,
            timeout_ms as usize,
            sigmask as usize,
            sigsetsize,
        ],
    )
}

pub fn sys_mkdirat(dirfd: isize, path: &str, mode: usize) -> isize {
    syscall(
        SYSCALL_MKDIRAT,
        [dirfd as usize, path.as_ptr() as usize, mode, 0, 0, 0],
    )
}

pub fn sys_unlinkat(dirfd: isize, path: &str, flags: usize) -> isize {
    syscall(
        SYSCALL_UNLINKAT,
        [dirfd as usize, path.as_ptr() as usize, flags, 0, 0, 0],
    )
}

pub fn sys_renameat2(
    olddirfd: isize,
    oldpath: &str,
    newdirfd: isize,
    newpath: &str,
    flags: usize,
) -> isize {
    syscall(
        SYSCALL_RENAMEAT2,
        [
            olddirfd as usize,
            oldpath.as_ptr() as usize,
            newdirfd as usize,
            newpath.as_ptr() as usize,
            flags,
            0,
        ],
    )
}

pub fn sys_truncate(path: &str, length: usize) -> isize {
    syscall(
        SYSCALL_TRUNCATE,
        [path.as_ptr() as usize, length, 0, 0, 0, 0],
    )
}

pub fn sys_ftruncate(fd: usize, length: usize) -> isize {
    syscall(SYSCALL_FTRUNCATE, [fd, length, 0, 0, 0, 0])
}

pub fn sys_fallocate(fd: usize, mode: usize, offset: isize, len: isize) -> isize {
    syscall(
        SYSCALL_FALLOCATE,
        [fd, mode, offset as usize, len as usize, 0, 0],
    )
}

pub fn sys_readlinkat(dirfd: isize, path: &str, buf: &mut [u8]) -> isize {
    syscall(
        SYSCALL_READLINKAT,
        [
            dirfd as usize,
            path.as_ptr() as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
            0,
            0,
        ],
    )
}

pub fn sys_rmdir(dirfd: isize, path: &str) -> isize {
    sys_unlinkat(dirfd, path, AT_REMOVEDIR)
}

pub fn sys_chdir(path: &str) -> isize {
    syscall(SYSCALL_CHDIR, [path.as_ptr() as usize, 0, 0, 0, 0, 0])
}

pub fn sys_fchmodat(dirfd: isize, path: &str, mode: usize, flags: usize) -> isize {
    syscall(
        SYSCALL_FCHMODAT,
        [dirfd as usize, path.as_ptr() as usize, mode, flags, 0, 0],
    )
}

pub fn sys_fchmod(fd: usize, mode: usize) -> isize {
    syscall(SYSCALL_FCHMOD, [fd, mode, 0, 0, 0, 0])
}

pub fn sys_fchownat(dirfd: isize, path: &str, owner: usize, group: usize, flags: usize) -> isize {
    syscall(
        SYSCALL_FCHOWNAT,
        [
            dirfd as usize,
            path.as_ptr() as usize,
            owner,
            group,
            flags,
            0,
        ],
    )
}

pub fn sys_fchown(fd: usize, owner: usize, group: usize) -> isize {
    syscall(SYSCALL_FCHOWN, [fd, owner, group, 0, 0, 0])
}

pub fn sys_utimensat(dirfd: isize, path: &str, times: &[TimeSpec; 2], flags: usize) -> isize {
    syscall(
        SYSCALL_UTIMENSAT,
        [
            dirfd as usize,
            path.as_ptr() as usize,
            times.as_ptr() as usize,
            flags,
            0,
            0,
        ],
    )
}

pub fn sys_futimens(fd: usize, times: &[TimeSpec; 2]) -> isize {
    syscall(SYSCALL_UTIMENSAT, [fd, 0, times.as_ptr() as usize, 0, 0, 0])
}

pub fn sys_openat(dirfd: isize, path: &str, flags: usize, mode: usize) -> isize {
    syscall(
        SYSCALL_OPENAT,
        [dirfd as usize, path.as_ptr() as usize, flags, mode, 0, 0],
    )
}

pub fn sys_mknodat(dirfd: isize, path: &str, mode: usize, dev: usize) -> isize {
    syscall(
        SYSCALL_MKNODAT,
        [dirfd as usize, path.as_ptr() as usize, mode, dev, 0, 0],
    )
}

pub fn sys_statx(dirfd: isize, path: &str, flags: usize, mask: u32, statx: &mut Statx) -> isize {
    syscall(
        SYSCALL_STATX,
        [
            dirfd as usize,
            path.as_ptr() as usize,
            flags,
            mask as usize,
            statx as *mut _ as usize,
            0,
        ],
    )
}

pub fn sys_fstatat(dirfd: isize, path: &str, stat: &mut Stat, flags: usize) -> isize {
    syscall(
        SYSCALL_FSTATAT,
        [
            dirfd as usize,
            path.as_ptr() as usize,
            stat as *mut _ as usize,
            flags,
            0,
            0,
        ],
    )
}

pub fn sys_faccessat2(dirfd: isize, path: &str, mode: usize, flags: usize) -> isize {
    syscall(
        SYSCALL_FACCESSAT2,
        [dirfd as usize, path.as_ptr() as usize, mode, flags, 0, 0],
    )
}

pub fn sys_setfsuid(uid: usize) -> isize {
    syscall(SYSCALL_SETFSUID, [uid, 0, 0, 0, 0, 0])
}

pub fn sys_setfsgid(gid: usize) -> isize {
    syscall(SYSCALL_SETFSGID, [gid, 0, 0, 0, 0, 0])
}

pub fn sys_getgroups(groups: &mut [u32]) -> isize {
    syscall(
        SYSCALL_GETGROUPS,
        [groups.len(), groups.as_mut_ptr() as usize, 0, 0, 0, 0],
    )
}

pub fn sys_setgroups(groups: &[u32]) -> isize {
    syscall(
        SYSCALL_SETGROUPS,
        [groups.len(), groups.as_ptr() as usize, 0, 0, 0, 0],
    )
}

pub fn sys_umask(mask: usize) -> isize {
    syscall(SYSCALL_UMASK, [mask, 0, 0, 0, 0, 0])
}

pub fn sys_close(fd: usize) -> isize {
    syscall(SYSCALL_CLOSE, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_setxattr(path: &str, name: &str, value: &[u8], flags: usize) -> isize {
    syscall(
        SYSCALL_SETXATTR,
        [
            path.as_ptr() as usize,
            name.as_ptr() as usize,
            value.as_ptr() as usize,
            value.len(),
            flags,
            0,
        ],
    )
}

pub fn sys_fsetxattr(fd: usize, name: &str, value: &[u8], flags: usize) -> isize {
    syscall(
        SYSCALL_FSETXATTR,
        [
            fd,
            name.as_ptr() as usize,
            value.as_ptr() as usize,
            value.len(),
            flags,
            0,
        ],
    )
}

pub fn sys_getxattr(path: &str, name: &str, value: &mut [u8]) -> isize {
    syscall(
        SYSCALL_GETXATTR,
        [
            path.as_ptr() as usize,
            name.as_ptr() as usize,
            value.as_mut_ptr() as usize,
            value.len(),
            0,
            0,
        ],
    )
}

pub fn sys_fgetxattr(fd: usize, name: &str, value: &mut [u8]) -> isize {
    syscall(
        SYSCALL_FGETXATTR,
        [
            fd,
            name.as_ptr() as usize,
            value.as_mut_ptr() as usize,
            value.len(),
            0,
            0,
        ],
    )
}

pub fn sys_listxattr(path: &str, list: &mut [u8]) -> isize {
    syscall(
        SYSCALL_LISTXATTR,
        [
            path.as_ptr() as usize,
            list.as_mut_ptr() as usize,
            list.len(),
            0,
            0,
            0,
        ],
    )
}

pub fn sys_removexattr(path: &str, name: &str) -> isize {
    syscall(
        SYSCALL_REMOVEXATTR,
        [path.as_ptr() as usize, name.as_ptr() as usize, 0, 0, 0, 0],
    )
}

pub fn sys_pipe2(pipefd: &mut [i32; 2], flags: usize) -> isize {
    syscall(
        SYSCALL_PIPE2,
        [pipefd.as_mut_ptr() as usize, flags, 0, 0, 0, 0],
    )
}

pub fn sys_getdents64(fd: usize, dirp: *mut u8, count: usize) -> isize {
    syscall(SYSCALL_GETDENTS64, [fd, dirp as usize, count, 0, 0, 0])
}

pub fn sys_lseek(fd: usize, offset: isize, whence: usize) -> isize {
    syscall(SYSCALL_LSEEK, [fd, offset as usize, whence, 0, 0, 0])
}

pub fn sys_stat(path: &str, stat: &mut Stat) -> isize {
    syscall(
        SYSCALL_FSTATAT,
        [
            crate::AT_FDCWD as usize,
            path.as_ptr() as usize,
            stat as *mut _ as usize,
            0,
            0,
            0,
        ],
    )
}

pub fn sys_fstat(fd: usize, stat: &mut Stat) -> isize {
    syscall(SYSCALL_FSTAT, [fd, stat as *mut _ as usize, 0, 0, 0, 0])
}

pub fn sys_fsync(fd: usize) -> isize {
    syscall(SYSCALL_FSYNC, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_sync() -> isize {
    syscall(SYSCALL_SYNC, [0; 6])
}

pub fn sys_syncfs(fd: usize) -> isize {
    syscall(SYSCALL_SYNCFS, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_fdatasync(fd: usize) -> isize {
    syscall(SYSCALL_FDATASYNC, [fd, 0, 0, 0, 0, 0])
}

pub fn sys_sync_file_range(fd: usize, offset: isize, nbytes: isize, flags: usize) -> isize {
    syscall(
        SYSCALL_SYNC_FILE_RANGE,
        [fd, offset as usize, nbytes as usize, flags, 0, 0],
    )
}

pub fn sys_timerfd_create(clockid: usize, flags: usize) -> isize {
    syscall(SYSCALL_TIMERFD_CREATE, [clockid, flags, 0, 0, 0, 0])
}

pub fn sys_memfd_create(name: &str, flags: usize) -> isize {
    syscall(
        SYSCALL_MEMFD_CREATE,
        [name.as_ptr() as usize, flags, 0, 0, 0, 0],
    )
}

pub fn sys_exit(exit_code: i32) -> isize {
    syscall(SYSCALL_EXIT, [exit_code as usize, 0, 0, 0, 0, 0])
}

pub fn sys_exit_group(exit_code: i32) -> isize {
    syscall(SYSCALL_EXIT_GROUP, [exit_code as usize, 0, 0, 0, 0, 0])
}

/// 主动交出 CPU 所有权
pub fn sys_sched_yield() -> isize {
    syscall(SYSCALL_SCHED_YIELD, [0, 0, 0, 0, 0, 0])
}

pub fn sys_sched_setaffinity(pid: isize, mask: &usize) -> isize {
    syscall(
        SYSCALL_SCHED_SETAFFINITY,
        [
            pid as usize,
            core::mem::size_of::<usize>(),
            mask as *const usize as usize,
            0,
            0,
            0,
        ],
    )
}

pub fn sys_sched_getaffinity(pid: isize, mask: &mut usize) -> isize {
    syscall(
        SYSCALL_SCHED_GETAFFINITY,
        [
            pid as usize,
            core::mem::size_of::<usize>(),
            mask as *mut usize as usize,
            0,
            0,
            0,
        ],
    )
}

pub fn sys_gettimeofday(tv: &mut TimeVal, tz: usize) -> isize {
    syscall(
        SYSCALL_GETTIMEOFDAY,
        [tv as *mut _ as usize, tz, 0, 0, 0, 0],
    )
}

pub fn sys_getuid() -> isize {
    syscall(SYSCALL_GETUID, [0, 0, 0, 0, 0, 0])
}

pub fn sys_getgid() -> isize {
    syscall(SYSCALL_GETGID, [0, 0, 0, 0, 0, 0])
}

pub fn sys_clone(flags: usize, stack: usize, ptid: usize, tls: usize, ctid: usize) -> isize {
    syscall(SYSCALL_CLONE, [flags, stack, ptid, tls, ctid, 0])
}

pub fn sys_execve(path: &str, args: &[*const u8], envp: *const *const u8) -> isize {
    syscall(
        SYSCALL_EXECVE,
        [
            path.as_ptr() as usize,
            args.as_ptr() as usize,
            envp as usize,
            0,
            0,
            0,
        ],
    )
}

pub fn sys_wait4(pid: isize, exit_code: *mut i32) -> isize {
    syscall(
        SYSCALL_WAIT4,
        [pid as usize, exit_code as usize, 0, 0, 0, 0],
    )
}

pub fn sys_wait4_options(pid: isize, exit_code: *mut i32, options: usize) -> isize {
    syscall(
        SYSCALL_WAIT4,
        [pid as usize, exit_code as usize, options, 0, 0, 0],
    )
}

pub fn sys_wait4_full(
    pid: isize,
    exit_code: *mut i32,
    options: usize,
    rusage: *mut RUsage,
) -> isize {
    syscall(
        SYSCALL_WAIT4,
        [
            pid as usize,
            exit_code as usize,
            options,
            rusage as usize,
            0,
            0,
        ],
    )
}

pub fn sys_getrusage(who: isize, usage: *mut RUsage) -> isize {
    syscall(
        SYSCALL_GETRUSAGE,
        [who as usize, usage as usize, 0, 0, 0, 0],
    )
}

pub fn sys_timer_create(clock_id: usize, timerid: *mut i32) -> isize {
    syscall(
        SYSCALL_TIMER_CREATE,
        [clock_id, 0, timerid as usize, 0, 0, 0],
    )
}

pub fn sys_timer_gettime(timerid: usize, current: *mut ITimerSpec) -> isize {
    syscall(
        SYSCALL_TIMER_GETTIME,
        [timerid, current as usize, 0, 0, 0, 0],
    )
}

pub fn sys_timer_settime(
    timerid: usize,
    flags: usize,
    new_value: *const ITimerSpec,
    old_value: *mut ITimerSpec,
) -> isize {
    syscall(
        SYSCALL_TIMER_SETTIME,
        [timerid, flags, new_value as usize, old_value as usize, 0, 0],
    )
}

pub fn sys_timer_delete(timerid: usize) -> isize {
    syscall(SYSCALL_TIMER_DELETE, [timerid, 0, 0, 0, 0, 0])
}

pub fn sys_clock_settime(clock_id: usize, value: *const TimeSpec) -> isize {
    syscall(
        SYSCALL_CLOCK_SETTIME,
        [clock_id, value as usize, 0, 0, 0, 0],
    )
}

pub fn sys_clock_gettime(clock_id: usize, value: *mut TimeSpec) -> isize {
    syscall(
        SYSCALL_CLOCK_GETTIME,
        [clock_id, value as usize, 0, 0, 0, 0],
    )
}

pub fn sys_clock_getres(clock_id: usize, value: *mut TimeSpec) -> isize {
    syscall(SYSCALL_CLOCK_GETRES, [clock_id, value as usize, 0, 0, 0, 0])
}

pub fn sys_prlimit64(
    pid: usize,
    resource: usize,
    new_limit: *const RLimit,
    old_limit: *mut RLimit,
) -> isize {
    syscall(
        SYSCALL_PRLIMIT64,
        [pid, resource, new_limit as usize, old_limit as usize, 0, 0],
    )
}

pub fn sys_futex(uaddr: *const u32, op: usize, val: usize, timeout: *const TimeSpec) -> isize {
    syscall(
        SYSCALL_FUTEX,
        [uaddr as usize, op, val, timeout as usize, 0, 0],
    )
}

pub fn sys_futex_full(
    uaddr: *const u32,
    op: usize,
    val: usize,
    val2: usize,
    uaddr2: *const u32,
    val3: usize,
) -> isize {
    syscall(
        SYSCALL_FUTEX,
        [uaddr as usize, op, val, val2, uaddr2 as usize, val3],
    )
}

pub fn sys_shmget(key: isize, size: usize, shmflg: usize) -> isize {
    syscall(SYSCALL_SHMGET, [key as usize, size, shmflg, 0, 0, 0])
}

pub fn sys_shmctl(shmid: usize, cmd: usize, buf: usize) -> isize {
    syscall(SYSCALL_SHMCTL, [shmid, cmd, buf, 0, 0, 0])
}

pub fn sys_shmat(shmid: usize, shmaddr: usize, shmflg: usize) -> isize {
    syscall(SYSCALL_SHMAT, [shmid, shmaddr, shmflg, 0, 0, 0])
}

pub fn sys_shmdt(shmaddr: usize) -> isize {
    syscall(SYSCALL_SHMDT, [shmaddr, 0, 0, 0, 0, 0])
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
        [
            signum as usize,
            action as usize,
            old_action as usize,
            core::mem::size_of::<u64>(),
            0,
            0,
        ],
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

pub fn sys_sigaction_raw(
    signum: i32,
    action: *const SignalAction,
    old_action: *mut SignalAction,
    sigsetsize: usize,
) -> isize {
    syscall(
        SYSCALL_SIGACTION,
        [
            signum as usize,
            action as usize,
            old_action as usize,
            sigsetsize,
            0,
            0,
        ],
    )
}

pub fn sys_sigprocmask(mask: u32) -> isize {
    syscall(SYSCALL_SIGPROCMASK, [mask as usize, 0, 0, 0, 0, 0])
}

pub fn sys_sigprocmask_raw(
    how: usize,
    set: *const u64,
    oldset: *mut u64,
    sigsetsize: usize,
) -> isize {
    syscall(
        SYSCALL_SIGPROCMASK,
        [how, set as usize, oldset as usize, sigsetsize, 0, 0],
    )
}

pub fn sys_sigpending_raw(set: *mut u64, sigsetsize: usize) -> isize {
    syscall(SYSCALL_SIGPENDING, [set as usize, sigsetsize, 0, 0, 0, 0])
}

pub fn sys_sigqueueinfo_raw(tgid: usize, signum: i32, info: *const u8) -> isize {
    syscall(
        SYSCALL_SIGQUEUEINFO,
        [tgid, signum as usize, info as usize, 0, 0, 0],
    )
}

pub fn sys_sigreturn() -> isize {
    syscall(SYSCALL_SIGRETURN, [0, 0, 0, 0, 0, 0])
}

pub fn sys_reboot() -> isize {
    syscall(SYSCALL_REBOOT, [0, 0, 0, 0, 0, 0])
}

pub fn sys_linkat(
    olddirfd: isize,
    oldpath: &str,
    newdirfd: isize,
    newpath: &str,
    flags: usize,
) -> isize {
    syscall(
        SYSCALL_LINKAT,
        [
            olddirfd as usize,
            oldpath.as_ptr() as usize,
            newdirfd as usize,
            newpath.as_ptr() as usize,
            flags,
            0,
        ],
    )
}

pub fn sys_symlinkat(target: &str, newdirfd: isize, linkpath: &str) -> isize {
    syscall(
        SYSCALL_SYMLINKAT,
        [
            target.as_ptr() as usize,
            newdirfd as usize,
            linkpath.as_ptr() as usize,
            0,
            0,
            0,
        ],
    )
}

pub fn sys_mount(source: &str, target: &str, fstype: &str, flags: usize, data: usize) -> isize {
    syscall(
        SYSCALL_MOUNT,
        [
            source.as_ptr() as usize,
            target.as_ptr() as usize,
            fstype.as_ptr() as usize,
            flags,
            data,
            0,
        ],
    )
}

pub fn sys_umount2(target: &str, flags: usize) -> isize {
    syscall(
        SYSCALL_UMOUNT2,
        [target.as_ptr() as usize, flags, 0, 0, 0, 0],
    )
}

pub fn sys_nanosleep(req: &TimeVal, rem: &mut TimeVal) -> isize {
    syscall(
        SYSCALL_NANOSLEEP,
        [req as *const _ as usize, rem as *mut _ as usize, 0, 0, 0, 0],
    )
}

pub fn sys_setpriority(which: usize, who: usize, prio: isize) -> isize {
    syscall(SYSCALL_SETPRIORITY, [which, who, prio as usize, 0, 0, 0])
}

pub fn sys_times(tms: &mut Tms) -> isize {
    syscall(SYSCALL_TIMES, [tms as *mut _ as usize, 0, 0, 0, 0, 0])
}

pub fn sys_setpgid(pid: isize, pgid: isize) -> isize {
    syscall(SYSCALL_SETPGID, [pid as usize, pgid as usize, 0, 0, 0, 0])
}

pub fn sys_getpgid(pid: isize) -> isize {
    syscall(SYSCALL_GETPGID, [pid as usize, 0, 0, 0, 0, 0])
}

pub fn sys_getsid(pid: isize) -> isize {
    syscall(SYSCALL_GETSID, [pid as usize, 0, 0, 0, 0, 0])
}

pub fn sys_setsid() -> isize {
    syscall(SYSCALL_SETSID, [0, 0, 0, 0, 0, 0])
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

pub fn sys_gettid() -> isize {
    syscall(SYSCALL_GETTID, [0, 0, 0, 0, 0, 0])
}

pub fn sys_socket(domain: usize, socket_type: usize, protocol: usize) -> isize {
    syscall(SYSCALL_SOCKET, [domain, socket_type, protocol, 0, 0, 0])
}

pub fn sys_socketpair(
    domain: usize,
    socket_type: usize,
    protocol: usize,
    fds: &mut [i32; 2],
) -> isize {
    syscall(
        SYSCALL_SOCKETPAIR,
        [
            domain,
            socket_type,
            protocol,
            fds.as_mut_ptr() as usize,
            0,
            0,
        ],
    )
}

pub fn sys_bind(fd: usize, addr: usize, addrlen: usize) -> isize {
    syscall(SYSCALL_BIND, [fd, addr, addrlen, 0, 0, 0])
}

pub fn sys_listen(fd: usize, backlog: usize) -> isize {
    syscall(SYSCALL_LISTEN, [fd, backlog, 0, 0, 0, 0])
}

pub fn sys_accept(fd: usize, addr: usize, addrlen: usize) -> isize {
    syscall(SYSCALL_ACCEPT, [fd, addr, addrlen, 0, 0, 0])
}

pub fn sys_connect(fd: usize, addr: usize, addrlen: usize) -> isize {
    syscall(SYSCALL_CONNECT, [fd, addr, addrlen, 0, 0, 0])
}

pub fn sys_getsockname(fd: usize, addr: usize, addrlen: usize) -> isize {
    syscall(SYSCALL_GETSOCKNAME, [fd, addr, addrlen, 0, 0, 0])
}

pub fn sys_getpeername(fd: usize, addr: usize, addrlen: usize) -> isize {
    syscall(SYSCALL_GETPEERNAME, [fd, addr, addrlen, 0, 0, 0])
}

pub fn sys_shutdown(fd: usize, how: usize) -> isize {
    syscall(SYSCALL_SHUTDOWN, [fd, how, 0, 0, 0, 0])
}

pub fn sys_ppoll(
    fds: *mut crate::PollFd,
    nfds: usize,
    timeout: *const TimeSpec,
    sigmask: *const u64,
    sigsetsize: usize,
) -> isize {
    syscall(
        SYSCALL_PPOLL,
        [
            fds as usize,
            nfds,
            timeout as usize,
            sigmask as usize,
            sigsetsize,
            0,
        ],
    )
}

pub fn sys_sendto(
    fd: usize,
    buf: *const u8,
    len: usize,
    flags: usize,
    addr: usize,
    addrlen: usize,
) -> isize {
    syscall(
        SYSCALL_SENDTO,
        [fd, buf as usize, len, flags, addr, addrlen],
    )
}

pub fn sys_recvfrom(
    fd: usize,
    buf: *mut u8,
    len: usize,
    flags: usize,
    addr: usize,
    addrlen: usize,
) -> isize {
    syscall(
        SYSCALL_RECVFROM,
        [fd, buf as usize, len, flags, addr, addrlen],
    )
}

pub fn sys_setsockopt(
    fd: usize,
    level: usize,
    optname: usize,
    optval: usize,
    optlen: usize,
) -> isize {
    syscall(SYSCALL_SETSOCKOPT, [fd, level, optname, optval, optlen, 0])
}

pub fn sys_getsockopt(
    fd: usize,
    level: usize,
    optname: usize,
    optval: usize,
    optlen: usize,
) -> isize {
    syscall(SYSCALL_GETSOCKOPT, [fd, level, optname, optval, optlen, 0])
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

pub fn sys_msync(addr: usize, len: usize, flags: i32) -> isize {
    syscall(SYSCALL_MSYNC, [addr, len, flags as usize, 0, 0, 0])
}

pub fn sys_mprotect(addr: usize, len: usize, prot: usize) -> isize {
    syscall(SYSCALL_MPROTECT, [addr, len, prot, 0, 0, 0])
}
