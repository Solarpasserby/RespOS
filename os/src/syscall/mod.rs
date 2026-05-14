// os/src/syscall.rs

//! ### 系统调用模块

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
const SYSCALL_GET_TIME: usize = 169;
const SYSCALL_FORK: usize     = 220;
const SYSCALL_EXEC: usize     = 221;
const SYSCALL_WAITPID: usize  = 260;
// FIXME: 把系统调用号按大小排布
const SYSCALL_KILL: usize     = 129;
const SYSCALL_SIGACTION: usize= 134;

mod fs;
mod process;
mod errno;

// 个人认为系统调用是提供给上层软件使用的
// 因此不对外暴露内部子函数
use fs::*;
use process::*;
pub use errno::*;

pub fn syscall(syscall_id: usize, args: [usize; 3]) -> SysResult<usize> {
    match syscall_id {
        SYSCALL_GETCWD   => sys_getcwd(args[0] as *mut u8, args[1]),
        SYSCALL_DUP      => sys_dup(args[0]),
        SYSCALL_DUP2     => sys_dup2(args[0], args[1]),
        SYSCALL_MKDIR    => sys_mkdir(args[0] as *const u8, args[1] as usize),
        SYSCALL_UNLINK   => sys_unlink(args[0] as *const u8),
        SYSCALL_CHDIR    => sys_chdir(args[0] as *const u8),
        SYSCALL_OPEN     => sys_open(args[0] as *const u8, args[1], args[2]),
        SYSCALL_CLOSE    => sys_close(args[0]),
        SYSCALL_PIPE     => sys_pipe(args[0] as *mut u32),
        SYSCALL_LSEEK    => sys_lseek(args[0], args[1] as isize, args[2]),
        SYSCALL_READ     => sys_read(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_WRITE    => sys_write(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_STAT     => sys_stat(args[0] as *const u8, args[1] as *mut crate::fs::Stat),
        SYSCALL_FSTAT    => sys_fstat(args[0], args[1] as *mut crate::fs::Stat),
        SYSCALL_EXIT     => sys_exit(args[0] as i32),
        SYSCALL_YIELD    => sys_yield(),
        SYSCALL_GET_TIME => sys_get_time(),
        SYSCALL_FORK     => sys_fork(),
        SYSCALL_EXEC     => sys_exec(args[0] as *const u8),
        SYSCALL_WAITPID  => sys_waitpid(args[0] as isize, args[1] as *mut i32),
        // FIXME: 这里同样按顺序排列
        _                => panic!("Unsupported syscall_id: {}", syscall_id),
    } 
}