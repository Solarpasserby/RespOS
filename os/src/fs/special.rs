use super::vfs::InodeType;
use super::{FileOp, KStat, OpenFlags};
use crate::syscall::{Errno, SysResult};
use core::any::Any;

pub struct SpecialFd {
    flags: OpenFlags,
    ty: InodeType,
    mode: u32,
}

impl SpecialFd {
    pub fn new(flags: OpenFlags, ty: InodeType) -> Self {
        Self {
            flags,
            ty,
            mode: match ty {
                InodeType::Regular => 0o600,
                _ => 0,
            },
        }
    }
}

impl FileOp for SpecialFd {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, _buf: &'a mut [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn write<'a>(&'a self, _buf: &'a [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn get_flags(&self) -> OpenFlags {
        self.flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        let stat = KStat::minimal(0, self.ty);
        if self.mode != 0 {
            Ok(stat.with_mode(self.mode))
        } else {
            Ok(stat)
        }
    }

    fn readable(&self) -> bool {
        false
    }

    fn writable(&self) -> bool {
        false
    }

    fn read_ready(&self) -> bool {
        false
    }
}
