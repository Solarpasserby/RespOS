// os/src/syscall/fs.rs

const FD_STDOUT: usize = 1;

/// 系统调用 `sys-write` ，向屏幕输出字符（并不准确）
/// 
/// 可以发现该系统调用需要访存，而此时处于内核态，无法直接获取用户态数据，需做简单处理
pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    match fd {
        FD_STDOUT => {
            let slice = unsafe { core::slice::from_raw_parts(buf, len) };
            let str = core::str::from_utf8(slice).unwrap();
            print!("{}",str);
            len as isize
        }
        _ => {
            panic!("Unsupported fd in sys_write!");
        }
    }
}