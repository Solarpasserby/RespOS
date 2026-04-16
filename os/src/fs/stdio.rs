// os/src/fs/stdio.rs

use core::any::Any;
use crate::sbi::console_getchar;
use crate::task::suspend_current_and_run_next;
use crate::syscall::SysResult;
use super::vfs::{FileOp, OpenFlags};

const LF: usize = 0x0a;
const CR: usize = 0x0d;

///Standard input
pub struct Stdin;
///Standard output
pub struct Stdout;

impl FileOp for Stdin {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        // 尝试支持读取多个字符
        let mut count: usize = 0;
        while count < buf.len() {
            let c = console_getchar();
            match c {
                // `c > 255`是为了兼容OPENSBI，OPENSBI未获取字符时会返回-1
                0 | 256.. => {
                    suspend_current_and_run_next();
                    continue;
                }
                CR | LF => {
                    buf[count] = LF as u8;
                    count += 1;
                    break;
                }
                _ => {
                    buf[count] = c as u8;
                    count += 1;
                }
            }
        }
        Ok(count)
    }
    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        panic!("Cannot write to stdin!");
    }
    fn seek(&self, _offset: isize) -> SysResult<usize> {
        panic!("Cannot seek stdin!");
    }
    fn get_offset(&self) -> usize {
        panic!("Cannot get offset from stdin!");
    }
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        false
    }
    fn get_flags(&self) -> OpenFlags {
        OpenFlags::empty()
    }
}

impl FileOp for Stdout {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn read(&self, _buf: &mut [u8]) -> SysResult<usize> {
        panic!("Cannot read from stdout!");
    }
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        unsafe {
            print!("{}", core::str::from_utf8_unchecked(buf));
        }
        Ok(buf.len())
    }
    fn seek(&self, _offset: isize) -> SysResult<usize> {
        panic!("Cannot seek stdin!");
    }
    fn get_offset(&self) -> usize {
        panic!("Cannot get offset from stdout!");
    }
    fn readable(&self) -> bool {
        false
    }
    fn writable(&self) -> bool {
        true
    }
    fn get_flags(&self) -> OpenFlags {
        OpenFlags::empty()
    }
}