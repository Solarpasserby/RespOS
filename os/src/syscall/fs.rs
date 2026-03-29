// os/src/syscall/fs.rs

use crate::mm::translate_byte_buffer;
use crate::task::{ current_user_token, suspend_current_and_run_next };
use crate::sbi::console_getchar;

const FD_STDIN: usize = 0;
const FD_STDOUT: usize = 1;

/// 系统调用 sys-read，读取字符
pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    match fd {
        FD_STDIN => {
            // 目前仅支持读取单个字符
            assert_eq!(len, 1, "Only support len = 1 in sys_read!");
            let mut c: usize;
            loop {
                c = console_getchar();
                if c == 0 {
                    suspend_current_and_run_next();
                    continue;
                } else { break; }
            }
            // 将读取的字符写入输出缓存
            let ch = c as u8;
            let mut buffers = translate_byte_buffer(current_user_token(), buf, len);
            unsafe { buffers[0].as_mut_ptr().write_volatile(ch); }
            1
        }
        _ => {
            panic!("[kernel] Unsupported fd: {} in sys_read!", fd);
        }
    }
}

/// 系统调用 sys-write，向屏幕输出字符（并不准确）
/// 
/// 可以发现该系统调用需要访存，而此时处于内核态，无法直接获取用户态数据，需做简单处理
pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    match fd {
        FD_STDOUT => {
            let token = current_user_token();
            let buffers = translate_byte_buffer(token, buf, len);
            for buffer in buffers {
                print!("{}", core::str::from_utf8(buffer).unwrap());
            }
            len as isize
        }
        _ => {
            panic!("[kernel] Unsupported fd: {} in sys_write!", fd);
        }
    }
}
