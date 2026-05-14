#![no_std]
#![feature(linkage)]
#![feature(alloc_error_handler)]

#[macro_use]
pub mod console;
mod lang_item;
mod syscall;

use bitflags::bitflags;
use buddy_system_allocator::LockedHeap;

const USER_HEAP_SIZE: usize = 4 * 4096;

static mut USER_SPACE: [u8; USER_HEAP_SIZE] = [0; USER_HEAP_SIZE];

#[global_allocator]
static HEAP: LockedHeap<32> = LockedHeap::empty();

#[alloc_error_handler]
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    clear_bss();
    unsafe {
        HEAP
            .lock()
            .init((&raw mut USER_SPACE) as usize, USER_HEAP_SIZE);
    }

    exit(main());
    panic!("unreachable after sys_exit!");
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
fn main() -> i32 {
    panic!("Cannot find main!");
}

fn clear_bss() {
    unsafe extern "C" {
        safe fn start_bss();
        safe fn end_bss();
    }
    (start_bss as *const() as usize..end_bss as *const() as usize).for_each(|addr| unsafe {
        (addr as *mut u8).write_volatile(0);
    });
}

use syscall::*;

pub use syscall::{Stat, TimeSpec};

pub const O_RDONLY: usize = 0;
pub const O_WRONLY: usize = 1 << 0;
pub const O_RDWR: usize = 1 << 1;
pub const O_CREATE: usize = 1 << 6;
pub const O_TRUNC: usize = 1 << 9;
pub const O_APPEND: usize = 1 << 10;
pub const O_DIRECTORY: usize = 1 << 16;

pub const SEEK_SET: usize = 0;
pub const SEEK_CUR: usize = 1;
pub const SEEK_END: usize = 2;

pub const SIGDEF: i32  = 0;  // 无信号，默认处理
pub const SIGHUP: i32  = 1;  // 挂起（终端断开）
pub const SIGINT: i32  = 2;  // Ctrl+C 中断
pub const SIGQUIT: i32 = 3;  // Ctrl+\ 退出
pub const SIGILL: i32  = 4;  // 非法指令
pub const SIGTRAP: i32 = 5;  // 调试断点
pub const SIGABRT: i32 = 6;  // 异常中止
pub const SIGBUS: i32  = 7;  // 总线错误（内存访问异常）
pub const SIGFPE: i32  = 8;  // 浮点异常（除0）
pub const SIGKILL: i32 = 9;  // 强制杀死进程（不能被屏蔽/捕获）
pub const SIGUSR1: i32 = 10; // 用户自定义信号1
pub const SIGSEGV: i32 = 11; // 段错误（非法内存访问）
pub const SIGUSR2: i32 = 12; // 用户自定义信号2
pub const SIGPIPE: i32 = 13; // 管道破裂
pub const SIGALRM: i32 = 14; // 时钟超时
pub const SIGTERM: i32 = 15; // 优雅终止
pub const SIGSTKFLT:i32= 16; // 协处理器栈错误
pub const SIGCHLD: i32 = 17; // 子进程退出
pub const SIGCONT: i32 = 18; // 继续运行
pub const SIGSTOP: i32 = 19; // 暂停进程（不能屏蔽）
pub const SIGTSTP: i32 = 20; // Ctrl+Z 暂停
pub const SIGTTIN: i32 = 21; // 后台进程读终端
pub const SIGTTOU: i32 = 22; // 后台进程写终端
pub const SIGURG: i32  = 23; // 紧急数据
pub const SIGXCPU: i32 = 24; // 超出CPU时间限制
pub const SIGXFSZ: i32 = 25; // 超出文件大小限制
pub const SIGVTALRM:i32= 26; // 虚拟时钟超时
pub const SIGPROF: i32 = 27; // 性能分析时钟
pub const SIGWINCH:i32= 28; // 窗口大小变化
pub const SIGIO: i32   = 29; // IO 就绪
pub const SIGPWR: i32  = 30; // 电源异常
pub const SIGSYS: i32  = 31; // 无效系统调用

pub fn read(fd: usize, buf: &mut [u8]) -> isize { sys_read(fd, buf) }
pub fn write(fd: usize, buf: &[u8]) -> isize { sys_write(fd, buf) }
pub fn getcwd(buf: &mut [u8]) -> isize { sys_getcwd(buf) }
pub fn dup(fd: usize) -> isize { sys_dup(fd) }
pub fn dup2(fd_src: usize, fd_dst: usize) -> isize { sys_dup2(fd_src, fd_dst) }
pub fn mkdir(path: &str, mode: usize) -> isize { sys_mkdir(path, mode) }
pub fn unlink(path: &str) -> isize { sys_unlink(path) }
pub fn chdir(path: &str) -> isize { sys_chdir(path) }
pub fn open(path: &str, flags: usize, mode: usize) -> isize { sys_open(path, flags, mode) }
pub fn close(fd: usize) -> isize { sys_close(fd) }
pub fn pipe(pipefd: &mut [u32; 2]) -> isize { sys_pipe(pipefd) }
pub fn lseek(fd: usize, offset: isize, whence: usize) -> isize {
    sys_lseek(fd, offset, whence)
}
pub fn stat(path: &str, stat: &mut Stat) -> isize { sys_stat(path, stat) }
pub fn fstat(fd: usize, stat: &mut Stat) -> isize { sys_fstat(fd, stat) }
pub fn exit(exit_code: i32) -> isize { sys_exit(exit_code) }
pub fn yield_() -> isize { sys_yield() }
pub fn time_get() -> isize { sys_get_time() }
pub fn fork() -> isize { sys_fork() }
pub fn exec(path: &str) -> isize { sys_exec(path) }
pub fn wait(exit_code: &mut i32) -> isize {
    loop { // 等待任意进程
        match sys_waitpid(-1, exit_code as *mut _) {
            -11 => { yield_(); } // 子进程未结束则让出资源
            exit_pid => return exit_pid,
        }
    }
}
pub fn waitpid(pid: usize, exit_code: &mut i32) -> isize {
    loop { // 等待指定进程
        match sys_waitpid(pid as isize, exit_code as *mut _) {
            -11 => { yield_(); } // 子进程未结束则让出资源
            exit_pid => return exit_pid,
        }
    }
}

pub fn kill(pid: usize, signum: i32) -> isize;

pub fn sigaction(
    signum: i32,
    action: Option<&SignalAction>,
    old_action: Option<&mut SignalAction>,
) -> isize {
    sys_sigaction(
        signum,
        action.map_or(core::ptr::null(), |a| a),
        old_action.map_or(core::ptr::null_mut(), |a: &mut SignalAction| a)
    )
}

pub struct SignalAction {
    pub handler: usize,
    pub mask : SignalFlags,
}

pub fn sigreturn() -> isize{
    sys_sigreturn()
}

pub fn sigprocmask(make: u32) -> isize{
    sys_sigprocmask
}

bitflags!{
    pub struct SignalFlags: i32{
        const SIGDEF    = 1 << 0;  // 0 号信号 → 1
        const SIGHUP    = 1 << 1;  // 1 号信号 → 2
        const SIGINT    = 1 << 2;  // 2 号信号 → 4
        const SIGQUIT   = 1 << 3;  // 3 号信号 → 8
        const SIGILL    = 1 << 4;  // 4 号信号 → 16
        const SIGTRAP   = 1 << 5;  // 5 号信号 → 32
        const SIGABRT   = 1 << 6;  // 6 号信号 → 64
        const SIGBUS    = 1 << 7;  // 7 号信号 → 128
        const SIGFPE    = 1 << 8;  // 8 号信号 → 256
        const SIGKILL   = 1 << 9;  // 9 号信号 → 512
        const SIGUSR1   = 1 << 10; // 10 号信号 → 1024
        const SIGSEGV   = 1 << 11; // 11 号信号 → 2048
        const SIGUSR2   = 1 << 12; // 12 号信号 → 4096
        const SIGPIPE   = 1 << 13; // 13 号信号 → 8192
        const SIGALRM   = 1 << 14; // 14 号信号 → 16384
        const SIGTERM   = 1 << 15; // 15 号信号 → 32768
        const SIGSTKFLT = 1 << 16; // 16 号信号 → 65536
        const SIGCHLD   = 1 << 17; // 17 号信号 → 131072
        const SIGCONT   = 1 << 18; // 18 号信号 → 262144
        const SIGSTOP   = 1 << 19; // 19 号信号 → 524288
        const SIGTSTP   = 1 << 20; // 20 号信号 → 1048576
        const SIGTTIN   = 1 << 21; // 21 号信号 → 2097152
        const SIGTTOU   = 1 << 22; // 22 号信号 → 4194304
        const SIGURG    = 1 << 23; // 23 号信号 → 8388608
        const SIGXCPU   = 1 << 24; // 24 号信号 → 16777216
        const SIGXFSZ   = 1 << 25; // 25 号信号 → 33554432
        const SIGVTALRM = 1 << 26; // 26 号信号 → 67108864
        const SIGPROF   = 1 << 27; // 27 号信号 → 134217728
        const SIGWINCH  = 1 << 28; // 28 号信号 → 268435456
        const SIGIO     = 1 << 29; // 29 号信号 → 536870912
        const SIGPWR    = 1 << 30; // 30 号信号 → 1073741824
        const SIGSYS    = 1 << 31; // 31 号信号 → 2147483648
    }
}


