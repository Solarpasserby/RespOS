// os/src/ext4/inode.rs

use alloc::ffi::CString;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::cell::SyncUnsafeCell;
use lwext4_rust::{Ext4File, InodeTypes as Ext4InodeTypes, bindings};

use crate::fs::KStat;
use crate::fs::vfs::{Dentry, InodeOp, InodeType, LinuxDirent64};
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

    fn file_link(&self, hardlink_path: &str) -> SysResult {
        let old_path = CString::new(self.abs_path.as_str()).map_err(|_| Errno::EINVAL)?;
        let new_path = CString::new(hardlink_path).map_err(|_| Errno::EINVAL)?;
        let ret = unsafe { bindings::ext4_flink(old_path.as_ptr(), new_path.as_ptr()) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }
        Ok(())
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

    fn dirent64_reclen(name_len: usize) -> usize {
        // 目录项固定字段大小
        const DIRENT64_HEADER_SIZE: usize = 8 + 8 + 2 + 1;
        // 变长文件名字段大小
        ((DIRENT64_HEADER_SIZE + name_len + 1) + 7) & !7 // 对齐 8 字节
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

    fn truncate(&self, size: usize) -> SysResult<usize> {
        self.check_type(InodeType::Regular)?;

        let file = self.inner();
        let path = file.get_path();
        let path = path.to_str().map_err(|_| Errno::EINVAL)?;

        file.file_open(path, bindings::O_RDWR)
            .map_err(Self::map_lwext4_err)?;
        file.file_truncate(size as u64)
            .map_err(Self::map_lwext4_err)?;
        file.file_close().map_err(Self::map_lwext4_err)?;

        Ok(0)
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

    fn readdir(&self) -> SysResult<Vec<LinuxDirent64>> {
        self.check_type(InodeType::Directory)?;

        let c_path = CString::new(self.abs_path.as_str()).map_err(|_| Errno::EINVAL)?;
        let c_path = c_path.into_raw();
        let mut dir: bindings::ext4_dir = unsafe { core::mem::zeroed() };
        let ret = unsafe { bindings::ext4_dir_open(&mut dir, c_path) };
        unsafe {
            drop(CString::from_raw(c_path));
        }
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
        }

        let mut entries = Vec::new();
        let mut next_off = 0usize;

        loop {
            let dirent = unsafe { bindings::ext4_dir_entry_next(&mut dir) };
            if dirent.is_null() {
                break;
            }

            let dirent = unsafe { &*dirent };
            let name_len = dirent.name_length as usize;
            let reclen = Self::dirent64_reclen(name_len);
            next_off += reclen;

            let mut d_name = dirent.name[..name_len].to_vec();
            d_name.push(0);
            entries.push(LinuxDirent64 {
                d_ino: dirent.inode as u64,
                d_off: next_off as i64,
                d_reclen: reclen as u16,
                d_type: InodeType::from(Ext4InodeTypes::from(dirent.inode_type as usize)) as u8,
                d_name,
            });
        }

        let ret = unsafe { bindings::ext4_dir_close(&mut dir) };
        if ret != 0 {
            return Err(Self::map_lwext4_err(ret));
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
                    .file_open(
                        &path,
                        bindings::O_RDWR | bindings::O_CREAT | bindings::O_TRUNC,
                    )
                    .map_err(Self::map_lwext4_err)?;
                new_file.file_close().map_err(Self::map_lwext4_err)?;
            }
            _ => return Err(Errno::ENOSYS),
        }

        Ok(Arc::new(new_inode))
    }

    fn link(&self, bare_dentry: Arc<Dentry>) -> SysResult {
        // 调用者保证参数合法
        // if self.node_type() == InodeType::Directory {
        //     return Err(Errno::EPERM);
        // }
        // if bare_dentry.try_get_inode().is_some() {
        //     return Err(Errno::EEXIST);
        // }

        self.file_link(&bare_dentry.abs_path)?;
        // TODO: 这里语义是新建了一个 inode，在优化 Ext4Inode 类型后需做修改
        bare_dentry.inner.lock().inode =
            Some(Arc::new(Self::new(&bare_dentry.abs_path, self.ty.clone())));
        Ok(())
    }

    fn unlink(&self, valid_dentry: Arc<Dentry>) -> SysResult {
        // 调用者保证参数合法
        // self.check_type(InodeType::Directory)?;

        let file = self.inner();
        info!("[kernel] unlink: {}", valid_dentry.abs_path);

        let child_abs_path = &valid_dentry.abs_path;
        let child_inode = valid_dentry.try_get_inode().ok_or(Errno::ENOENT)?;
        if child_inode.node_type() == InodeType::Directory {
            let entries = child_inode.readdir()?;
            if !entries.is_empty() {
                return Err(Errno::ENOTEMPTY);
            }
            file.dir_rm(child_abs_path).map_err(Self::map_lwext4_err)?;
        } else {
            // lwext4_rust 包中 `file_remove` 的语义是 unlink 而非删除文件
            file.file_remove(child_abs_path)
                .map_err(Self::map_lwext4_err)?;
        };
        Ok(())
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
            Ext4InodeTypes::EXT4_DE_FIFO | Ext4InodeTypes::EXT4_INODE_MODE_FIFO => InodeType::Fifo,
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
