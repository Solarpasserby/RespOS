#![no_std]
#![feature(linkage)]
#![feature(alloc_error_handler)]

#[macro_use]
pub mod console;
mod lang_item;
mod syscall;

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
pub fn pipe(pipefd: &mut [usize; 2]) -> isize { sys_pipe(pipefd) }
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
