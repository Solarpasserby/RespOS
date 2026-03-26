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
static HEAP: LockedHeap = LockedHeap::empty();

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

pub fn read(fd: usize, buf: &mut [u8]) -> isize { sys_read(fd, buf) }
pub fn write(fd: usize, buf: &[u8]) -> isize { sys_write(fd, buf) }
pub fn exit(exit_code: i32) -> isize { sys_exit(exit_code) }
pub fn yield_() -> isize { sys_yield() }
pub fn time_get() -> isize { sys_get_time() }
pub fn fork() -> isize { sys_fork() }
pub fn exec(path: &str) -> isize { sys_exec(path) }
pub fn wait(exit_code: &mut i32) -> isize {
    loop { // 等待任意进程
        match sys_waitpid(-1, exit_code as *mut _) {
            -2 => { yield_(); } // 子进程未结束则让出资源
            exit_pid => return exit_pid,
        }
    }
}
pub fn waitpid(pid: usize, exit_code: &mut i32) -> isize {
    loop { // 等待指定进程
        match sys_waitpid(pid as isize, exit_code as *mut _) {
            -2 => { yield_(); } // 子进程未结束则让出资源
            exit_pid => return exit_pid,
        }
    }
}