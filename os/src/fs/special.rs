use super::vfs::InodeType;
use super::{FileOp, KStat, OpenFlags};
use crate::syscall::{Errno, SysResult};
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

const F_SEAL_SEAL: usize = 0x0001;
const F_SEAL_SHRINK: usize = 0x0002;
const F_SEAL_GROW: usize = 0x0004;
const F_SEAL_WRITE: usize = 0x0008;
const F_SEAL_MASK: usize = F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE;

pub struct SpecialFd {
    flags: OpenFlags,
    ty: InodeType,
    mode: u32,
    offset: Mutex<usize>,
    data: Option<Mutex<Vec<u8>>>,
    seals: Mutex<usize>,
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
            offset: Mutex::new(0),
            data: None,
            seals: Mutex::new(0),
        }
    }

    pub fn new_memfd(flags: OpenFlags, allow_sealing: bool) -> Self {
        Self {
            flags,
            ty: InodeType::Regular,
            mode: 0o600,
            offset: Mutex::new(0),
            data: Some(Mutex::new(Vec::new())),
            seals: Mutex::new(if allow_sealing { 0 } else { F_SEAL_SEAL }),
        }
    }

    pub fn seals(&self) -> usize {
        *self.seals.lock()
    }

    pub fn add_seals(&self, seals: usize) -> SysResult<usize> {
        if seals & !F_SEAL_MASK != 0 {
            return Err(Errno::EINVAL);
        }
        let mut current = self.seals.lock();
        if *current & F_SEAL_SEAL != 0 {
            return Err(Errno::EPERM);
        }
        *current |= seals;
        Ok(0)
    }
}

impl FileOp for SpecialFd {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        let data = self.data.as_ref().ok_or(Errno::EINVAL)?.lock();
        let mut offset = self.offset.lock();
        if *offset >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - *offset);
        buf[..n].copy_from_slice(&data[*offset..*offset + n]);
        *offset += n;
        Ok(n)
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        if self.seals() & F_SEAL_WRITE != 0 {
            return Err(Errno::EPERM);
        }
        let mut data = self.data.as_ref().ok_or(Errno::EINVAL)?.lock();
        let mut offset = self.offset.lock();
        let end = offset.checked_add(buf.len()).ok_or(Errno::EINVAL)?;
        if end > data.len() {
            if self.seals() & F_SEAL_GROW != 0 {
                return Err(Errno::EPERM);
            }
            data.resize(end, 0);
        }
        data[*offset..end].copy_from_slice(buf);
        *offset = end;
        Ok(buf.len())
    }

    fn can_seek(&self) -> SysResult {
        if self.data.is_some() {
            Ok(())
        } else {
            Err(Errno::ESPIPE)
        }
    }

    fn seek(&self, offset: isize) -> SysResult<usize> {
        self.can_seek()?;
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        *self.offset.lock() = offset;
        Ok(offset)
    }

    fn get_offset(&self) -> usize {
        *self.offset.lock()
    }

    fn get_flags(&self) -> OpenFlags {
        self.flags
    }

    fn get_stat(&self) -> SysResult<KStat> {
        let size = self
            .data
            .as_ref()
            .map(|data| data.lock().len())
            .unwrap_or(0);
        let stat = KStat::minimal(size, self.ty);
        if self.mode != 0 {
            Ok(stat.with_mode(self.mode))
        } else {
            Ok(stat)
        }
    }

    fn readable(&self) -> bool {
        self.data.is_some() && !self.flags.contains(OpenFlags::O_WRONLY)
    }

    fn writable(&self) -> bool {
        self.data.is_some()
            && self
                .flags
                .intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    }

    fn read_ready(&self) -> bool {
        self.data.is_some()
    }

    fn truncate(&self, size: usize) -> SysResult<usize> {
        let mut data = self.data.as_ref().ok_or(Errno::EINVAL)?.lock();
        let old_len = data.len();
        let seals = self.seals();
        if size < old_len && seals & F_SEAL_SHRINK != 0 {
            return Err(Errno::EPERM);
        }
        if size > old_len && seals & F_SEAL_GROW != 0 {
            return Err(Errno::EPERM);
        }
        data.resize(size, 0);
        Ok(0)
    }
}
