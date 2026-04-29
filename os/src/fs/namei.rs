// os/src/fs/namei.rs

use alloc::{
    format,
    vec::Vec,
    sync::Arc,
};
use crate::task::current_task;
use crate::syscall::{SysResult, Errno};
use super::vfs::{Dentry, ROOT_DENTRY, File, OpenFlags, InodeType};

/// 路径名解析状态量
pub struct Nameidata<'a> {
    // pub mnt: Arc<VfsMount>,
    pub dentry: Arc<Dentry>,
    path_segments: Vec<&'a str>,
    depth: usize,
}

impl<'a> Nameidata<'a> {
    pub fn new(file_name: &'a str) -> Self {
        let path_segments: Vec<&'a str> = file_name.split('/').filter(|s| !s.is_empty()).collect();
        let task = current_task().expect("[kernel] current task is None.");
        let (_, dentry) = if file_name.starts_with("/") {
            (0, ROOT_DENTRY.clone())
        } else {
            let path = task.cwd();
            (0, path.dentry.clone())
        };

        Nameidata {
            dentry,
            path_segments,
            depth: 0,
        }
    }
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
    let child_dentry = Arc::new(Dentry::new(
        abs_path,
        Some(nd.dentry.clone()),
        child_inode,
    ));
    nd.dentry.insert_child(name, child_dentry.clone());

    Ok(child_dentry)
}

// TODO: 个人觉得创建子目录项的过程不合适，之后实现缓存的时候再做修改
fn install_child_dentry(
    parent: &Arc<Dentry>,
    name: &str,
    inode: Arc<dyn super::vfs::InodeOp>,
) -> Arc<Dentry> {
    let abs_path = if parent.abs_path == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent.abs_path, name)
    };

    let child_dentry = Arc::new(Dentry::new(
        abs_path,
        Some(parent.clone()),
        inode,
    ));
    parent.insert_child(name, child_dentry.clone());
    child_dentry
}

pub fn open_last_lookups(
    nd: &mut Nameidata,
    flags: usize,
    _mode: usize, // TODO: mode 变量未被使用
) -> SysResult<Arc<File>> {
    // 目标为根目录或工作目录
    if nd.path_segments.is_empty() {
        return Ok(Arc::new(File::new(nd.dentry.get_inode(), OpenFlags::from(flags))));
    }

    let name = nd.path_segments[nd.depth];
    let flags = OpenFlags::from(flags);

    let inode = if name == "." {
        nd.dentry.get_inode()
    } else if name == ".." {
        let parent_dentry = nd.dentry.get_parent_or_self();
        parent_dentry.get_inode()
    } else {
        match lookup_dentry(nd) {
            // 成功
            Ok(dentry) => {
                let inode = dentry.get_inode();
                // 期望打开目录，但实际文件类型不是目录，返回错误
                if flags.contains(OpenFlags::O_DIRECTORY) && inode.node_type() != InodeType::Directory {
                    return Err(Errno::ENOTDIR);
                }
                // TODO: 此处默认 dentry 不是负目录项，在引入缓存后需修改
                inode
            },
            Err(Errno::ENOENT) if flags.contains(OpenFlags::O_CREATE) => {
                // 期望打开目录，但目标不存在
                if flags.contains(OpenFlags::O_DIRECTORY) {
                    return Err(Errno::ENOTDIR);
                }
                let current_dir_inode = nd.dentry.get_inode();
                let inode = current_dir_inode.create(name, InodeType::Regular)?;
                install_child_dentry(&nd.dentry, name, inode.clone());
                inode
            },
            Err(err) => return Err(err),
        }
    };

    Ok(Arc::new(File::new(inode, flags)))
}

/// 根据路径打开文件
pub fn path_open(path: &str, flags: usize, mode: usize) -> SysResult<Arc<File>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let mut nd = Nameidata::new(path);
    link_path_walk(&mut nd)?;
    open_last_lookups(&mut nd, flags, mode)
}

/// 根据路径创建文件
pub fn filename_create(path: &str, ty: InodeType, _mode: usize) -> SysResult {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let mut nd = Nameidata::new(path);
    link_path_walk(&mut nd)?;

    // 目标为根目录或工作目录，返回错误
    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }
    let name = nd.path_segments[nd.depth];
    if name == "." || name == ".." {
        Err(Errno::EEXIST)
    } else {
        // TODO: 引入负目录项需进行修改，这里先做简单实现
        match lookup_dentry(&mut nd) {
            Ok(_) => Err(Errno::EEXIST),
            // 未找到目标文件，创建文件
            Err(Errno::ENOENT) => {
                let current_dir_inode = nd.dentry.get_inode();
                let inode = current_dir_inode.create(name, ty)?;
                install_child_dentry(&nd.dentry, name, inode);
                Ok(())
            },
            Err(err) => Err(err),
        }
    }
}

/// 根据路径查询文件
pub fn filename_lookup(path: &str, _mode: usize) -> SysResult<Arc<Dentry>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }   
    let mut nd = Nameidata::new(path);
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

/// 路径解析主函数，循环解析每一层，定位到最后的目标
pub fn link_path_walk(nd: &mut Nameidata) -> SysResult {
    // TDOD: 未处理符号连接，连续解析路径的情况。主要这个函数被多次使用，我把未实现的提示搬到这里
    println!("[kernel] func:link_path_walk path: {:?}", nd.path_segments);
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
