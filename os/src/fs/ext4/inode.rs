// os/src/ext4/inode.rs

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::cell::SyncUnsafeCell;
use lwext4_rust::{bindings, Ext4File, InodeTypes as Ext4InodeTypes};

use crate::fs::KStat;
use crate::fs::vfs::{DirEntry, InodeOp, InodeType};
use crate::syscall::{Errno, SysResult};

pub struct Ext4Inode {
    abs_path: String,
    ty: Ext4InodeTypes,
    inner: SyncUnsafeCell<Ext4File>,
}

unsafe impl Send for Ext4Inode {}
unsafe impl Sync for Ext4Inode {}

impl Ext4Inode {
    pub fn new(path: &str, ty: Ext4InodeTypes) -> Self {
        Self {
            abs_path: path.to_string(),
            ty: ty.clone(),
            inner: SyncUnsafeCell::new(Ext4File::new(path, ty)),
        }
    }

    fn inner(&self) -> &mut Ext4File {
        unsafe { &mut *self.inner.get() }
    }

    fn child_path(&self, name: &str) -> String {
        if self.abs_path == "/" {
            alloc::format!("/{}", name)
        } else {
            alloc::format!("{}/{}", self.abs_path, name)
        }
    }

    fn dirent_name_eq(raw_name: &[u8], expected: &str) -> bool {
        let len = raw_name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(raw_name.len());
        raw_name[..len] == *expected.as_bytes()
    }

    fn check_type(&self, expected: InodeType) -> SysResult<()> {
        let actual = self.node_type();
        if actual == expected {
            Ok(())
        } else if expected == InodeType::Directory {
            Err(Errno::ENOTDIR)
        } else if actual == InodeType::Directory {
            Err(Errno::EISDIR)
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn map_lwext4_err(errno: i32) -> Errno {
        match errno {
            2 => Errno::ENOENT,
            5 => Errno::EIO,
            17 => Errno::EEXIST,
            20 => Errno::ENOTDIR,
            21 => Errno::EISDIR,
            22 => Errno::EINVAL,
            28 => Errno::ENOSPC,
            30 => Errno::EROFS,
            39 => Errno::ENOTEMPTY,
            _ => Errno::EIO,
        }
    }

    fn file_size(&self) -> SysResult<usize> {
        let file = self.inner();
        let path = file.get_path();
        let path = path.to_str().map_err(|_| Errno::EINVAL)?;

        file.file_open(path, bindings::O_RDONLY)
            .map_err(Self::map_lwext4_err)?;
        let size = file.file_size() as usize;
        file.file_close().map_err(Self::map_lwext4_err)?;
        Ok(size)
    }
}

impl InodeOp for Ext4Inode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn node_type(&self) -> InodeType {
        InodeType::from(self.ty.clone())
    }

    fn stat(&self) -> SysResult<KStat> {
        let ty = self.node_type();
        let size = if ty == InodeType::Regular {
            self.file_size()?
        } else {
            0
        };
        Ok(KStat { size, ty })
    }

    fn read_at(&self, off: usize, buf: &mut [u8]) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;

        let file = self.inner();
        let path = file.get_path();
        let path = path.to_str().map_err(|_| Errno::EINVAL)?;

        file.file_open(path, bindings::O_RDONLY)
            .map_err(Self::map_lwext4_err)?;
        file.file_seek(off as i64, bindings::SEEK_SET)
            .map_err(Self::map_lwext4_err)?;
        let read_size = file.file_read(buf).map_err(Self::map_lwext4_err)?;
        file.file_close().map_err(Self::map_lwext4_err)?;

        Ok(read_size)
    }

    fn write_at(&self, off: usize, buf: &[u8]) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;

        let file = self.inner();
        let path = file.get_path();
        let path = path.to_str().map_err(|_| Errno::EINVAL)?;

        file.file_open(path, bindings::O_RDWR)
            .map_err(Self::map_lwext4_err)?;
        file.file_seek(off as i64, bindings::SEEK_SET)
            .map_err(Self::map_lwext4_err)?;
        let write_size = file.file_write(buf).map_err(Self::map_lwext4_err)?;
        file.file_close().map_err(Self::map_lwext4_err)?;

        Ok(write_size)
    }

    /// 查找与 name 匹配的子索引节点，约定 name 为常规文件名
    fn lookup(&self, name: &str) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;

        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(Errno::EINVAL);
        }

        let file = self.inner();
        let (names, types) = file.lwext4_dir_entries().map_err(Self::map_lwext4_err)?;
        let child_ty = names
            .iter()
            .zip(types.into_iter())
            .find_map(|(entry_name, entry_ty)| {
                Self::dirent_name_eq(entry_name, name).then_some(entry_ty)
            })
            .ok_or(Errno::ENOENT)?;

        let child_path = self.child_path(name);
        Ok(Arc::new(Self::new(&child_path, child_ty)))
    }

    fn readdir(&self) -> SysResult<Vec<DirEntry>> {
        self.check_type(InodeType::Directory)?;

        let file = self.inner();
        let (names, types) = file.lwext4_dir_entries().map_err(Self::map_lwext4_err)?;
        let mut entries = Vec::with_capacity(names.len());

        for (name, ty) in names.into_iter().zip(types.into_iter()) {
            let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            let name = core::str::from_utf8(&name[..len]).map_err(|_| Errno::EINVAL)?;
            entries.push(DirEntry {
                name: name.to_string(),
                ty: InodeType::from(ty),
            });
        }

        Ok(entries)
    }

    fn create(&self, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>> {
        self.check_type(InodeType::Directory)?;

        let path = self.child_path(name);
        let ext4_ty = Ext4InodeTypes::from(ty);
        let file = self.inner();

        if file.check_inode_exist(&path, ext4_ty.clone()) {
            return Err(Errno::EEXIST);
        }

        let new_inode = Self::new(&path, ext4_ty.clone());
        let new_file = new_inode.inner();

        match ext4_ty {
            Ext4InodeTypes::EXT4_DE_DIR => {
                new_file.dir_mk(&path).map_err(Self::map_lwext4_err)?;
            }
            Ext4InodeTypes::EXT4_DE_REG_FILE => {
                new_file
                    .file_open(&path, bindings::O_RDWR | bindings::O_CREAT | bindings::O_TRUNC)
                    .map_err(Self::map_lwext4_err)?;
                new_file.file_close().map_err(Self::map_lwext4_err)?;
            }
            _ => return Err(Errno::ENOSYS),
        }

        Ok(Arc::new(new_inode))
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let _ = self.inner().file_close();
    }
}

impl From<InodeType> for Ext4InodeTypes {
    fn from(ty: InodeType) -> Self {
        match ty {
            InodeType::Unknown => Ext4InodeTypes::EXT4_DE_UNKNOWN,
            InodeType::Fifo => Ext4InodeTypes::EXT4_DE_FIFO,
            InodeType::CharDevice => Ext4InodeTypes::EXT4_DE_CHRDEV,
            InodeType::Directory => Ext4InodeTypes::EXT4_DE_DIR,
            InodeType::BlockDevice => Ext4InodeTypes::EXT4_DE_BLKDEV,
            InodeType::Regular => Ext4InodeTypes::EXT4_DE_REG_FILE,
            InodeType::SymLink => Ext4InodeTypes::EXT4_DE_SYMLINK,
            InodeType::Socket => Ext4InodeTypes::EXT4_DE_SOCK,
        }
    }
}

impl From<Ext4InodeTypes> for InodeType {
    fn from(ty: Ext4InodeTypes) -> Self {
        match ty {
            Ext4InodeTypes::EXT4_DE_UNKNOWN => InodeType::Unknown,
            Ext4InodeTypes::EXT4_DE_FIFO | Ext4InodeTypes::EXT4_INODE_MODE_FIFO => {
                InodeType::Fifo
            }
            Ext4InodeTypes::EXT4_DE_CHRDEV | Ext4InodeTypes::EXT4_INODE_MODE_CHARDEV => {
                InodeType::CharDevice
            }
            Ext4InodeTypes::EXT4_DE_DIR | Ext4InodeTypes::EXT4_INODE_MODE_DIRECTORY => {
                InodeType::Directory
            }
            Ext4InodeTypes::EXT4_DE_BLKDEV | Ext4InodeTypes::EXT4_INODE_MODE_BLOCKDEV => {
                InodeType::BlockDevice
            }
            Ext4InodeTypes::EXT4_DE_REG_FILE | Ext4InodeTypes::EXT4_INODE_MODE_FILE => {
                InodeType::Regular
            }
            Ext4InodeTypes::EXT4_DE_SYMLINK | Ext4InodeTypes::EXT4_INODE_MODE_SOFTLINK => {
                InodeType::SymLink
            }
            Ext4InodeTypes::EXT4_DE_SOCK | Ext4InodeTypes::EXT4_INODE_MODE_SOCKET => {
                InodeType::Socket
            }
            _ => InodeType::Unknown,
        }
    }
}
