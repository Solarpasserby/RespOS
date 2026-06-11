// os/src/fs/namei.rs

use super::Path;
use super::dentry_cache::{insert_dentry_cache, lookup_dentry_cache, remove_dentry_cache};
use super::mount::{VfsMount, get_mount_by_dentry, get_mount_by_vfsmount, root_path};
use super::vfs::{Dentry, InodeType};
use super::{File, OpenFlags};
use crate::fs::ext4::Ext4Inode;
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use alloc::{format, string::String, sync::Arc, vec::Vec};

pub const AT_FDCWD: isize = -100;
pub const AT_NO_AUTOMOUNT: usize = 0x800;
pub const AT_EMPTY_PATH: usize = 0x1000;
pub const AT_SYMLINK_NOFOLLOW: usize = 0x100;

const MAX_SYMLINK_FOLLOWS: usize = 40;

/// 路径名解析状态量
pub struct Nameidata<'a> {
    pub mnt: Arc<VfsMount>,
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

        let base = if file_name.starts_with("/") {
            root_path()
        } else {
            base
        };

        Nameidata {
            mnt: base.mnt.clone(),
            dentry: base.dentry.clone(),
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
/// 先查全局 DentryCache，未命中再调文件系统 lookup
pub fn lookup_dentry(nd: &mut Nameidata) -> SysResult<Arc<Dentry>> {
    lookup_dentry_maybe_follow_mount(nd, true)
}

fn lookup_dentry_maybe_follow_mount(
    nd: &mut Nameidata,
    follow_mount: bool,
) -> SysResult<Arc<Dentry>> {
    let name = nd.path_segments[nd.depth];
    let abs_path = child_abs_path(&nd.dentry, name);

    // 查询全局 DentryCache
    if let Some(cached) = lookup_dentry_cache(&abs_path) {
        if follow_mount {
            if let Some(mount) = get_mount_by_dentry(&cached) {
                nd.mnt = mount.vfs_mount.clone();
                nd.dentry = mount.vfs_mount.root.clone();
                return Ok(nd.dentry.clone());
            }
        }
        return Ok(cached);
    }

    // 缓存未命中，调文件系统 lookup
    let current_dir_inode = nd.dentry.get_inode();
    let child_inode = current_dir_inode.lookup(&nd.dentry.abs_path, name)?;

    // 创建新 dentry，建立父子关系，加入缓存
    let child_dentry = Arc::new(Dentry::new(abs_path, Some(nd.dentry.clone()), child_inode));
    nd.dentry.insert_child(name, child_dentry.clone());
    insert_dentry_cache(child_dentry.clone());

    if follow_mount {
        if let Some(mount) = get_mount_by_dentry(&child_dentry) {
            nd.mnt = mount.vfs_mount.clone();
            nd.dentry = mount.vfs_mount.root.clone();
            return Ok(nd.dentry.clone());
        }
    }

    Ok(child_dentry)
}

fn follow_dotdot(nd: &mut Nameidata) {
    if Arc::ptr_eq(&nd.dentry, &nd.mnt.root) {
        if let Some(mount) = get_mount_by_vfsmount(&nd.mnt) {
            if let Some(parent) = mount.parent.as_ref().and_then(|parent| parent.upgrade()) {
                nd.mnt = parent.vfs_mount.clone();
                nd.dentry = mount.mountpoint.get_parent_or_self();
                return;
            }
        }
    }
    nd.dentry = nd.dentry.get_parent_or_self();
}

// 创建子目录项的过程不合适
fn install_child_dentry(
    parent: &Arc<Dentry>,
    name: &str,
    inode: Arc<dyn super::vfs::InodeOp>,
) -> Arc<Dentry> {
    let abs_path = child_abs_path(parent, name);
    let child_dentry = Arc::new(Dentry::new(abs_path, Some(parent.clone()), inode));
    parent.insert_child(name, child_dentry.clone());
    insert_dentry_cache(child_dentry.clone());
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
    let flags = OpenFlags::from(flags);

    // 目标为根目录或工作目录
    if nd.path_segments.is_empty() {
        let path = Path::new(nd.mnt.clone(), nd.dentry.clone());
        let inode = nd.dentry.get_inode();
        if flags.contains(OpenFlags::O_TMPFILE) {
            if inode.node_type() != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }
            return Ok(Arc::new(File::new_tmpfile(path, inode, flags)));
        }
        return Ok(Arc::new(File::new(path, inode, flags)));
    }

    let name = nd.path_segments[nd.depth];

    let dentry = if name == "." {
        nd.dentry.clone()
    } else if name == ".." {
        follow_dotdot(nd);
        nd.dentry.clone()
    } else {
        match lookup_dentry(nd) {
            // 成功
            Ok(dentry) => {
                let inode = dentry.get_inode();
                if flags.contains(OpenFlags::O_TMPFILE) {
                    if inode.node_type() != InodeType::Directory {
                        return Err(Errno::ENOTDIR);
                    }
                    return Ok(Arc::new(File::new_tmpfile(
                        Path::new(nd.mnt.clone(), dentry),
                        inode,
                        flags,
                    )));
                }
                if flags.contains(OpenFlags::O_CREATE | OpenFlags::O_EXCL) {
                    return Err(Errno::EEXIST);
                }
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
                let inode =
                    current_dir_inode.create(&nd.dentry.abs_path, name, InodeType::Regular)?;
                install_child_dentry(&nd.dentry, name, inode.clone())
            }
            Err(err) => return Err(err),
        }
    };

    let inode = dentry.get_inode();
    Ok(Arc::new(File::new(
        Path::new(nd.mnt.clone(), dentry),
        inode,
        flags,
    )))
}

/// 根据路径打开文件
pub fn path_open(dirfd: isize, path: &str, flags: usize, mode: usize) -> SysResult<Arc<File>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let open_flags = OpenFlags::from(flags);
    if !open_flags.contains(OpenFlags::O_CREATE) {
        let path = filename_lookup(dirfd, path, 0)?;
        let inode = path.dentry.get_inode();
        if open_flags.contains(OpenFlags::O_TMPFILE) {
            if inode.node_type() != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }
            return Ok(Arc::new(File::new_tmpfile(path, inode, open_flags)));
        }
        if open_flags.contains(OpenFlags::O_DIRECTORY) && inode.node_type() != InodeType::Directory
        {
            return Err(Errno::ENOTDIR);
        }
        return Ok(Arc::new(File::new(path, inode, open_flags)));
    }

    // TODO[ABI-COMPAT]: O_CREAT 路径仍沿用旧的 parent-walk 流程；
    // 若最后一级已经存在且是符号链接，Linux 默认应继续跟随到目标。
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
            let inode = current_dir_inode.create(&nd.dentry.abs_path, name, ty)?;
            install_child_dentry(&nd.dentry, name, inode);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

pub fn filename_symlink(dirfd: isize, target: &str, newpath: &str) -> SysResult {
    if target.is_empty() || newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    // symlinkat 的 linkpath 需要按“父目录 + 新名字”解析：
    // 中间路径仍正常解析，最后一级必须不存在。
    let mut nd = Nameidata::new_at(dirfd, newpath)?;
    link_path_walk(&mut nd)?;

    // 不能把根目录或当前工作目录本身替换成一个新的符号链接目录项。
    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth];
    // "." 和 ".." 不是普通文件名，不能作为新符号链接的名字。
    if name == "." || name == ".." {
        return Err(Errno::EEXIST);
    }

    match lookup_dentry(&mut nd) {
        Ok(_) => Err(Errno::EEXIST),
        Err(Errno::ENOENT) => {
            let parent_inode = nd.dentry.get_inode();
            // 目标字符串原样写入 symlink inode；相对路径到真正解析时再以链接所在目录为基准解释。
            let inode = parent_inode.symlink(target, &nd.dentry.abs_path, name)?;
            install_child_dentry(&nd.dentry, name, inode);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// 根据路径查询文件
pub fn filename_lookup(dirfd: isize, path: &str, _mode: usize) -> SysResult<Arc<Path>> {
    // 默认 lookup 语义和 Linux 一致：跟随最后一级符号链接，也允许穿过最终挂载点。
    resolve_path(dirfd, path, true, true)
}

pub fn filename_lookup_no_follow_final_mount(dirfd: isize, path: &str) -> SysResult<Arc<Path>> {
    // umount 等场景需要定位挂载点本身，因此最后一级不能穿过 mount。
    resolve_path(dirfd, path, true, false)
}

pub fn filename_lookup_no_follow_final_symlink(dirfd: isize, path: &str) -> SysResult<Arc<Path>> {
    // readlinkat/lstat 需要拿到 symlink inode 自身；中间路径中的 symlink 仍要正常跟随。
    resolve_path(dirfd, path, false, true)
}

fn resolve_path(
    dirfd: isize,
    path: &str,
    follow_final_symlink: bool,
    follow_final_mount: bool,
) -> SysResult<Arc<Path>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    // dirfd 只决定相对路径的起点；绝对路径会在 Nameidata::new_from_path 中切回 root_path。
    let base = base_path_from_dirfd(dirfd, path)?;
    resolve_path_from(base, path, follow_final_symlink, follow_final_mount, 0)
}

fn resolve_path_from(
    base: Arc<Path>,
    path: &str,
    follow_final_symlink: bool,
    follow_final_mount: bool,
    symlink_follows: usize,
) -> SysResult<Arc<Path>> {
    // 限制 symlink 展开次数，防止 a -> b -> a 这种循环把内核拖进无限递归。
    if symlink_follows > MAX_SYMLINK_FOLLOWS {
        return Err(Errno::ELOOP);
    }

    let mut nd = Nameidata::new_from_path(path, base);
    // 空 segment 表示路径就是起点本身，例如 "/" 或相对路径中的空字符串。
    if nd.path_segments.is_empty() {
        return Ok(Path::new(nd.mnt.clone(), nd.dentry.clone()));
    }

    while nd.depth < nd.path_segments.len() {
        let name = nd.path_segments[nd.depth];
        let is_last = nd.depth + 1 == nd.path_segments.len();

        // "." 不改变当前位置；如果它是最后一级，直接返回当前 path。
        if name == "." {
            if is_last {
                return Ok(Path::new(nd.mnt.clone(), nd.dentry.clone()));
            }
            nd.depth += 1;
            continue;
        }

        // ".." 需要考虑 mount root，follow_dotdot 会在跨挂载点时退回父 mount 的挂载点。
        if name == ".." {
            follow_dotdot(&mut nd);
            if is_last {
                return Ok(Path::new(nd.mnt.clone(), nd.dentry.clone()));
            }
            nd.depth += 1;
            continue;
        }

        // 如果 child 是相对 symlink，后续解析必须以“链接所在目录”为起点。
        // 因此先保存当前目录 path，再去 lookup child。
        let symlink_base = Path::new(nd.mnt.clone(), nd.dentry.clone());
        // 中间路径必须穿过 mount；最后一级是否穿过 mount 由调用者决定。
        let child = lookup_dentry_maybe_follow_mount(&mut nd, !is_last || follow_final_mount)?;
        let child_path = Path::new(nd.mnt.clone(), child.clone());
        let inode = child.get_inode();

        // 中间 symlink 一定要展开；最后一级是否展开由 follow_final_symlink 决定。
        if inode.node_type() == InodeType::SymLink && (!is_last || follow_final_symlink) {
            let target = inode.read_link(&child_path.abs_path())?;
            // target 替换当前 symlink，其余未解析 segment 继续拼到 target 后面。
            let next_path = join_symlink_target(&target, &nd.path_segments[nd.depth + 1..]);
            let next_base = if target.starts_with('/') {
                // 绝对 symlink 目标从全局 root 开始解析。
                root_path()
            } else {
                // 相对 symlink 目标从 symlink 所在目录开始解析。
                symlink_base
            };
            return resolve_path_from(
                next_base,
                &next_path,
                follow_final_symlink,
                follow_final_mount,
                symlink_follows + 1,
            );
        }

        if is_last {
            // 最后一级不是需要展开的 symlink，当前 child 就是解析结果。
            return Ok(child_path);
        }

        nd.dentry = child;
        nd.depth += 1;
    }

    Ok(Path::new(nd.mnt.clone(), nd.dentry.clone()))
}

fn join_symlink_target(target: &str, rest: &[&str]) -> String {
    let mut path = String::from(target);
    for segment in rest {
        // 保留 target 本身的绝对/相对形式，只负责把剩余 segment 接到后面。
        if path.is_empty() || path.ends_with('/') {
            path.push_str(segment);
        } else {
            path.push('/');
            path.push_str(segment);
        }
    }
    path
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
    parent.get_inode().unlink(&target)?;
    parent.remove_child(name.as_str());
    remove_dentry_cache(&target.abs_path);
    Ok(())
}

/// 根据两个路径创建硬链接。
pub fn filename_link(olddirfd: isize, oldpath: &str, newdirfd: isize, newpath: &str) -> SysResult {
    if oldpath.is_empty() || newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    let old_path = filename_lookup(olddirfd, oldpath, 0)?;
    if old_path.dentry.get_inode().node_type() == InodeType::Directory {
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
            let old_inode = old_path.dentry.get_inode();
            old_inode.link(&old_path.abs_path(), bare_dentry.clone())?;
            bare_dentry.inner.lock().inode = Some(old_inode);
            nd.dentry.insert_child(name, bare_dentry.clone());
            insert_dentry_cache(bare_dentry);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// 路径基于 rename 系统调用。将 oldpath 重命名为 newpath 指定的新路径。
///
/// 仅处理新旧路径同在一个文件系统上的情况。
pub fn filename_rename(
    olddirfd: isize,
    oldpath: &str,
    newdirfd: isize,
    newpath: &str,
) -> SysResult {
    if oldpath.is_empty() || newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    let old = filename_lookup(olddirfd, oldpath, 0)?;

    let mut nd = Nameidata::new_at(newdirfd, newpath)?;
    link_path_walk(&mut nd)?;

    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth];
    if name == "." || name == ".." {
        return Err(Errno::EBUSY);
    }

    let new_abs = child_abs_path(&nd.dentry, name);

    // 防止 ext4_frename 在目标为非空目录时产生嵌套语义，损坏文件系统。
    if let Ok(existing) = lookup_dentry(&mut nd) {
        let existing_ty = existing.get_inode().node_type();
        if existing_ty != InodeType::Directory {
            // 目标为非目录文件：交由 ext4_frename 替换
        } else {
            let entries = existing.get_inode().readdir(&existing.abs_path)?;
            let has_content = entries
                .iter()
                .any(|e| e.d_name != b".\0" && e.d_name != b"..\0");
            if has_content {
                return Err(Errno::ENOTEMPTY);
            }
        }
    }

    Ext4Inode::file_rename(&old.abs_path(), &new_abs)?;

    // 更新 VFS dentry 树：Arc<Dentry> 可能已被多个 Path/File 共享，不能原地可变修改。
    // 这里为新路径创建新的 dentry，并从旧父目录移除旧名字。
    let old_parent = old
        .dentry
        .get_parent()
        .unwrap_or_else(|| old.dentry.clone());
    old_parent.remove_child(old.dentry.abs_path.rsplit('/').next().unwrap_or(""));

    remove_dentry_cache(&old.abs_path().as_str());
    remove_dentry_cache(&new_abs);
    let renamed_dentry = Arc::new(Dentry::new(
        new_abs,
        Some(nd.dentry.clone()),
        old.dentry.get_inode(),
    ));
    nd.dentry.insert_child(name, renamed_dentry.clone());
    insert_dentry_cache(renamed_dentry);

    Ok(())
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
            follow_dotdot(nd);
        } else {
            let child_dentry = lookup_dentry(nd)?;
            nd.dentry = child_dentry;
        }
        // 统一推进解析深度
        nd.depth += 1
    }
    Ok(())
}
