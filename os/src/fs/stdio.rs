// os/src/fs/stdio.rs

use super::KStat;
use super::vfs::InodeType;
use super::{FileOp, OpenFlags};
use crate::syscall::{Errno, SysResult};
use core::any::Any;

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
        crate::fs::tty::read_console(buf, crate::fs::tty::ConsoleReadKind::Stdin)
    }
    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        panic!("Cannot write to stdin!");
    }
    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }
    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }
    fn get_offset(&self) -> usize {
        0
    }
    fn readable(&self) -> bool {
        true
    }
    fn read_ready(&self) -> bool {
        crate::fs::tty::console_read_ready()
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
        crate::fs::tty::check_background_write()?;
        crate::fs::tty::write_console(buf);
        Ok(buf.len())
    }
    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }
    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }
    fn get_offset(&self) -> usize {
        0
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
