#![no_std]
#![feature(linkage)]
#![feature(alloc_error_handler)]

#[macro_use]
pub mod console;
mod lang_item;
#[allow(unused)]
mod syscall;

extern crate alloc;

use buddy_system_allocator::LockedHeap;

const USER_HEAP_SIZE: usize = 8 * 4096;
const USER_ARG_MAX_COUNT: usize = 32;

static mut USER_SPACE: [u8; USER_HEAP_SIZE] = [0; USER_HEAP_SIZE];

#[global_allocator]
static HEAP: LockedHeap<32> = LockedHeap::empty();

#[alloc_error_handler]
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start(argc: usize, argv: usize) -> ! {
    clear_bss();
    unsafe {
        HEAP
            .lock()
            .init((&raw mut USER_SPACE) as usize, USER_HEAP_SIZE);
    }

    let mut argv_ref: [&str; USER_ARG_MAX_COUNT] = [""; USER_ARG_MAX_COUNT];
    for i in 0..argc {
        let str_start = unsafe {
            ((argv + i * core::mem::size_of::<usize>()) as *const usize).read_volatile()
        };
        let len = (0usize..).find(|i| unsafe {
            ((str_start + *i) as *const u8).read_volatile() == 0
        }).unwrap();
        argv_ref[i] = core::str::from_utf8(unsafe {
            core::slice::from_raw_parts(str_start as *const u8, len)
        }).unwrap();
    }
    
    exit(main(argc, &argv_ref[..argc]));
    panic!("unreachable after sys_exit!");
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
fn main(_argc: usize, _argv: &[&str]) -> i32 {
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

pub use syscall::{Stat, TimeSpec, TimeVal};

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
pub const AT_FDCWD: isize = -100;

pub fn read(fd: usize, buf: &mut [u8]) -> isize { sys_read(fd, buf) }
pub fn write(fd: usize, buf: &[u8]) -> isize { sys_write(fd, buf) }
pub fn getcwd(buf: &mut [u8]) -> isize { sys_getcwd(buf) }
pub fn dup(fd: usize) -> isize { sys_dup(fd) }
pub fn dup2(fd_src: usize, fd_dst: usize) -> isize { sys_dup3(fd_src, fd_dst, 0) }
pub fn mkdir(path: &str, mode: usize) -> isize { sys_mkdirat(AT_FDCWD, path, mode) }
pub fn unlink(path: &str) -> isize { sys_unlinkat(AT_FDCWD, path, 0) }
pub fn chdir(path: &str) -> isize { sys_chdir(path) }
pub fn open(path: &str, flags: usize, mode: usize) -> isize { sys_openat(AT_FDCWD, path, flags, mode) }
pub fn close(fd: usize) -> isize { sys_close(fd) }
pub fn pipe(pipefd: &mut [usize; 2]) -> isize { sys_pipe2(pipefd, 0) }
pub fn getdents64(fd: usize, buf: &mut [u8]) -> isize {
    sys_getdents64(fd, buf.as_mut_ptr(), buf.len())
}
pub fn lseek(fd: usize, offset: isize, whence: usize) -> isize {
    sys_lseek(fd, offset, whence)
}
pub fn stat(path: &str, stat: &mut Stat) -> isize { sys_stat(path, stat) }
pub fn fstat(fd: usize, stat: &mut Stat) -> isize { sys_fstat(fd, stat) }
pub fn exit(exit_code: i32) -> isize { sys_exit(exit_code) }
pub fn yield_() -> isize { sys_sched_yield() }
pub fn time_get() -> isize {
    let mut tv = TimeVal::default();
    match sys_gettimeofday(&mut tv, 0) {
        0 => (tv.sec * 1000 + tv.usec / 1000) as isize,
        err => err,
    }
}
pub fn fork() -> isize { sys_clone(17, 0, 0, 0, 0) }
pub fn exec(path: &str, args: &[*const u8]) -> isize { sys_execve(path, args, &[]) }
pub fn wait(exit_code: &mut i32) -> isize {
    loop { // 等待任意进程
        match sys_wait4(-1, exit_code as *mut _, 0, 0) {
            -11 => { yield_(); } // 子进程未结束则让出资源
            exit_pid => return exit_pid,
        }
    }
}
pub fn waitpid(pid: usize, exit_code: &mut i32) -> isize {
    loop { // 等待指定进程
        match sys_wait4(pid as isize, exit_code as *mut _, 0, 0) {
            -11 => { yield_(); } // 子进程未结束则让出资源
            exit_pid => return exit_pid,
        }
    }
}
