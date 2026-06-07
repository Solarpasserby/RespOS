// os/src/fs/stdio.rs

use super::KStat;
use super::vfs::{FileOp, InodeType, OpenFlags};
use crate::sbi::console_getchar;
use crate::syscall::SysResult;
use crate::task::yield_current_task;
use core::any::Any;

const LF: usize = 0x0a;
const CR: usize = 0x0d;

///Standard input
pub struct Stdin;
///Standard output
pub struct Stdout;

const STDIN_INO: u64 = 0x2000;
const STDOUT_INO: u64 = 0x2001;
const STDIO_DEV: u64 = 0x300;
const CONSOLE_RDEV: u64 = (5 << 8) | 1;

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
                    yield_current_task();
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
    fn is_tty(&self) -> bool {
        true
    }
    fn get_flags(&self) -> OpenFlags {
        OpenFlags::empty()
    }
    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::CharDevice)
            .with_dev(STDIO_DEV)
            .with_ino(STDIN_INO)
            .with_mode(0o666)
            .with_rdev(CONSOLE_RDEV))
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
    fn is_tty(&self) -> bool {
        true
    }
    fn get_flags(&self) -> OpenFlags {
        OpenFlags::empty()
    }
    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::CharDevice)
            .with_dev(STDIO_DEV)
            .with_ino(STDOUT_INO)
            .with_mode(0o666)
            .with_rdev(CONSOLE_RDEV))
    }
}
