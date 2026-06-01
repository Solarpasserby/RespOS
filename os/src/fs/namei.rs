// os/src/fs/namei.rs

use super::Path;
use super::vfs::{Dentry, File, InodeType, OpenFlags, ROOT_DENTRY};
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use alloc::{format, string::String, sync::Arc, vec::Vec};

pub const AT_FDCWD: isize = -100;

/// 路径名解析状态量
pub struct Nameidata<'a> {
    // pub mnt: Arc<VfsMount>,
    pub dentry: Arc<Dentry>,
    path_segments: Vec<&'a str>,
    depth: usize,
}

impl<'a> Nameidata<'a> {
    pub fn new(file_name: &'a str) -> Self {
        let task = current_task().expect("[kernel] current task is None.");
        Self::new_from_path(file_name, task.cwd())
    }

    pub fn new_at(dirfd: isize, file_name: &'a str) -> SysResult<Self> {
        let base = base_path_from_dirfd(dirfd, file_name)?;
        Ok(Self::new_from_path(file_name, base))
    }

    fn new_from_path(file_name: &'a str, base: Arc<Path>) -> Self {
        let path_segments: Vec<&'a str> = file_name.split('/').filter(|s| !s.is_empty()).collect();

        let dentry = if file_name.starts_with("/") {
            ROOT_DENTRY.clone()
        } else {
            base.dentry.clone()
        };

        Nameidata {
            dentry,
            path_segments,
            depth: 0,
        }
    }
}

fn base_path_from_dirfd(dirfd: isize, file_name: &str) -> SysResult<Arc<Path>> {
    let task = current_task().expect("[kernel] current task is None.");
    if file_name.starts_with('/') || dirfd == AT_FDCWD {
        return Ok(task.cwd());
    }
    if dirfd < 0 {
        return Err(Errno::EBADF);
    }

    let file = task.get_fd_entry(dirfd as usize)?.get_file();
    let file = file.as_any().downcast_ref::<File>().ok_or(Errno::ENOTDIR)?;
    if file.inode().node_type() != InodeType::Directory {
        return Err(Errno::ENOTDIR);
    }
    Ok(file.path())
}

/// 根据当前 `Nameidate` 查找下一级目录项
///
/// 首先查找 Dentry 的孩子，然后查找 DentryCache，最后查找 Inode 的子节点
pub fn lookup_dentry(nd: &mut Nameidata) -> SysResult<Arc<Dentry>> {
    let name = nd.path_segments[nd.depth];

    // 查找当前 Dentry 的孩子，找到直接返回
    if let Some(child) = nd.dentry.get_child(name) {
        return Ok(child);
    }

    let abs_path = if nd.dentry.abs_path == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", nd.dentry.abs_path, name)
    };
    // TODO: 查询 DentryCache

    // 获取当前 Dentry 的 Inode
    let current_dir_inode = nd.dentry.get_inode();
    // 查找 Inode 的子节点
    let child_inode = current_dir_inode.lookup(name)?;

    // 创建新的 Dentry，并更新父子状态。TODO: 将新建 Dentry 加入缓存
    let child_dentry = Arc::new(Dentry::new(abs_path, Some(nd.dentry.clone()), child_inode));
    nd.dentry.insert_child(name, child_dentry.clone());

    Ok(child_dentry)
}

// TODO: 个人觉得创建子目录项的过程不合适，之后实现缓存的时候再做修改
fn install_child_dentry(
    parent: &Arc<Dentry>,
    name: &str,
    inode: Arc<dyn super::vfs::InodeOp>,
) -> Arc<Dentry> {
    let abs_path = child_abs_path(parent, name);
    let child_dentry = Arc::new(Dentry::new(abs_path, Some(parent.clone()), inode));
    parent.insert_child(name, child_dentry.clone());
    child_dentry
}

fn child_abs_path(parent: &Arc<Dentry>, name: &str) -> alloc::string::String {
    if parent.abs_path == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.abs_path, name)
    }
}

fn dentry_name(dentry: &Arc<Dentry>) -> SysResult<String> {
    dentry
        .abs_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(String::from)
        .ok_or(Errno::EBUSY)
}

pub fn open_last_lookups(
    nd: &mut Nameidata,
    flags: usize,
    _mode: usize, // TODO: mode 变量未被使用
) -> SysResult<Arc<File>> {
    // 目标为根目录或工作目录
    if nd.path_segments.is_empty() {
        return Ok(Arc::new(File::new(
            Path::new(nd.dentry.clone()),
            nd.dentry.get_inode(),
            OpenFlags::from(flags),
        )));
    }

    let name = nd.path_segments[nd.depth];
    let flags = OpenFlags::from(flags);

    let dentry = if name == "." {
        nd.dentry.clone()
    } else if name == ".." {
        nd.dentry.get_parent_or_self()
    } else {
        match lookup_dentry(nd) {
            // 成功
            Ok(dentry) => {
                let inode = dentry.get_inode();
                // 期望打开目录，但实际文件类型不是目录，返回错误
                if flags.contains(OpenFlags::O_DIRECTORY)
                    && inode.node_type() != InodeType::Directory
                {
                    return Err(Errno::ENOTDIR);
                }
                // TODO: 此处默认 dentry 不是负目录项，在引入缓存后需修改
                dentry
            }
            Err(Errno::ENOENT) if flags.contains(OpenFlags::O_CREATE) => {
                // 期望打开目录，但目标不存在
                if flags.contains(OpenFlags::O_DIRECTORY) {
                    return Err(Errno::ENOTDIR);
                }
                let current_dir_inode = nd.dentry.get_inode();
                let inode = current_dir_inode.create(name, InodeType::Regular)?;
                install_child_dentry(&nd.dentry, name, inode.clone())
            }
            Err(err) => return Err(err),
        }
    };

    let inode = dentry.get_inode();
    Ok(Arc::new(File::new(Path::new(dentry), inode, flags)))
}

/// 根据路径打开文件
pub fn path_open(dirfd: isize, path: &str, flags: usize, mode: usize) -> SysResult<Arc<File>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let mut nd = Nameidata::new_at(dirfd, path)?;
    link_path_walk(&mut nd)?;
    open_last_lookups(&mut nd, flags, mode)
}

/// 根据路径创建文件
pub fn filename_create(dirfd: isize, path: &str, ty: InodeType, _mode: usize) -> SysResult {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let mut nd = Nameidata::new_at(dirfd, path)?;
    link_path_walk(&mut nd)?;

    // 目标为根目录或工作目录，返回错误
    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth];
    if name == "." || name == ".." {
        return Err(Errno::EEXIST);
    }

    // TODO: 引入负目录项需进行修改，这里先做简单实现
    match lookup_dentry(&mut nd) {
        Ok(_) => Err(Errno::EEXIST),
        // 未找到目标文件，创建文件
        Err(Errno::ENOENT) => {
            let current_dir_inode = nd.dentry.get_inode();
            let inode = current_dir_inode.create(name, ty)?;
            install_child_dentry(&nd.dentry, name, inode);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// 根据路径查询文件
pub fn filename_lookup(dirfd: isize, path: &str, _mode: usize) -> SysResult<Arc<Dentry>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let mut nd = Nameidata::new_at(dirfd, path)?;
    link_path_walk(&mut nd)?;

    // 目标为根目录或工作目录
    if nd.path_segments.is_empty() {
        return Ok(nd.dentry.clone());
    }

    let name = nd.path_segments[nd.depth];
    if name == "." {
        Ok(nd.dentry.clone())
    } else if name == ".." {
        let parent_dentry = nd.dentry.get_parent_or_self();
        Ok(parent_dentry)
    } else {
        lookup_dentry(&mut nd)
    }
}

/// 根据路径删除一个目录项。
///
/// `remove_dir == false` 只能删除非目录；
/// `remove_dir == true` 只能删除目录。
pub fn filename_unlink(dirfd: isize, path: &str, remove_dir: bool) -> SysResult {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }

    let mut nd = Nameidata::new_at(dirfd, path)?;
    link_path_walk(&mut nd)?;

    // 目标为根目录或工作目录，不允许删除目录项
    if nd.path_segments.is_empty() {
        return if remove_dir {
            Err(Errno::EBUSY)
        } else {
            Err(Errno::EISDIR)
        };
    }

    // 获取目标 dentry
    let name = nd.path_segments[nd.depth];
    if name == "." || name == ".." {
        return if remove_dir {
            Err(Errno::EINVAL)
        } else {
            Err(Errno::EISDIR)
        };
    }

    let target = lookup_dentry(&mut nd)?;
    let target_ty = target.get_inode().node_type();
    if target_ty == InodeType::Directory && !remove_dir {
        return Err(Errno::EISDIR);
    }
    if target_ty != InodeType::Directory && remove_dir {
        return Err(Errno::ENOTDIR);
    }

    let parent = target.get_parent().ok_or(Errno::EBUSY)?;
    let name = dentry_name(&target)?;
    parent.get_inode().unlink(target)?;
    parent.remove_child(name.as_str());
    Ok(())
}

/// 根据两个路径创建硬链接。
pub fn filename_link(olddirfd: isize, oldpath: &str, newdirfd: isize, newpath: &str) -> SysResult {
    if oldpath.is_empty() || newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    let old_dentry = filename_lookup(olddirfd, oldpath, 0)?;
    if old_dentry.get_inode().node_type() == InodeType::Directory {
        return Err(Errno::EPERM);
    }

    let mut nd = Nameidata::new_at(newdirfd, newpath)?;
    link_path_walk(&mut nd)?;

    // 目标为根目录或工作目录
    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth];
    if name == "." || name == ".." {
        return Err(Errno::EEXIST);
    }

    match lookup_dentry(&mut nd) {
        Ok(_) => Err(Errno::EEXIST),
        Err(Errno::ENOENT) => {
            let bare_dentry =
                Dentry::negative(child_abs_path(&nd.dentry, name), Some(nd.dentry.clone()));
            old_dentry.get_inode().link(bare_dentry.clone())?;
            nd.dentry.insert_child(name, bare_dentry);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// 路径解析主函数，循环解析每一层，定位到最后的目标
pub fn link_path_walk(nd: &mut Nameidata) -> SysResult {
    // TDOD: 未处理符号连接，连续解析路径的情况。主要这个函数被多次使用，我把未实现的提示搬到这里
    // println!("[kernel] func:link_path_walk path: {:?}", nd.path_segments);
    if nd.path_segments.is_empty() {
        return Ok(());
    }
    let len = nd.path_segments.len() - 1;
    while nd.depth < len {
        let name = nd.path_segments[nd.depth];
        if name == "." {
            //do nothing
        } else if name == ".." {
            let parent_dentry = nd.dentry.get_parent_or_self();
            nd.dentry = parent_dentry;
        } else {
            let child_dentry = lookup_dentry(nd)?;
            nd.dentry = child_dentry;
        }
        // 统一推进解析深度
        nd.depth += 1
    }
    Ok(())
}
