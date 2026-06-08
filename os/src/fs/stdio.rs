// os/src/fs/stdio.rs

use super::KStat;
use super::vfs::{FileOp, InodeType, OpenFlags};
use crate::sbi::console_getchar;
use crate::syscall::{Errno, SysResult};
use crate::task::yield_current_task;
use core::any::Any;

const LF: usize = 0x0a;
const CR: usize = 0x0d;

///Standard input
pub struct Stdin;
///Standard output
pub struct Stdout;
/// Null character device.
pub struct DevNull {
    flags: OpenFlags,
}
/// Zero character device.
pub struct DevZero {
    flags: OpenFlags,
}

impl DevNull {
    pub fn new(flags: OpenFlags) -> Self {
        Self { flags }
    }
}

impl DevZero {
    pub fn new(flags: OpenFlags) -> Self {
        Self { flags }
    }
}

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
        Err(Errno::ESPIPE)
    }
    fn get_offset(&self) -> usize {
        0
    }
    fn can_seek(&self) -> SysResult<()> {
        Err(Errno::ESPIPE)
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
    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat {
            size: 0,
            ty: InodeType::CharDevice,
        })
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
        Err(Errno::ESPIPE)
    }
    fn get_offset(&self) -> usize {
        0
    }
    fn can_seek(&self) -> SysResult<()> {
        Err(Errno::ESPIPE)
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
    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat {
            size: 0,
            ty: InodeType::CharDevice,
        })
    }
}

impl FileOp for DevNull {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, _buf: &'a mut [u8]) -> SysResult<usize> {
        Ok(0)
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn can_seek(&self) -> SysResult<()> {
        Err(Errno::ESPIPE)
    }

    fn readable(&self) -> bool {
        !self.flags.contains(OpenFlags::O_WRONLY)
    }

    fn writable(&self) -> bool {
        self.flags
            .intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    }

    fn get_flags(&self) -> OpenFlags {
        self.flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat {
            size: 0,
            ty: InodeType::CharDevice,
        })
    }
}

impl FileOp for DevZero {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn can_seek(&self) -> SysResult<()> {
        Err(Errno::ESPIPE)
    }

    fn readable(&self) -> bool {
        !self.flags.contains(OpenFlags::O_WRONLY)
    }

    fn writable(&self) -> bool {
        self.flags
            .intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    }

    fn get_flags(&self) -> OpenFlags {
        self.flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat {
            size: 0,
            ty: InodeType::CharDevice,
        })
    }
}
