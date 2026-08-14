// os/src/fs/namei.rs

use super::Path;
use super::dentry_cache::{insert_dentry_cache, lookup_dentry_cache, remove_dentry_cache_tree};
use super::mount::{
    MS_NODEV, MS_NOSYMFOLLOW, VfsMount, get_mount_by_dentry, get_mount_by_vfsmount, root_path,
};
use super::vfs::{Dentry, InodeType};
use super::{File, OpenFlags, TmpFileMeta};
use crate::fs::ext4::Ext4Inode;
use crate::fs::register_dirty_owner;
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use crate::timer::{TimeSpec, get_time_ms};
use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use spin::Mutex;

pub const AT_FDCWD: isize = -100;
pub const AT_NO_AUTOMOUNT: usize = 0x800;
pub const AT_EMPTY_PATH: usize = 0x1000;
pub const AT_SYMLINK_NOFOLLOW: usize = 0x100;

const MAX_SYMLINK_FOLLOWS: usize = 40;
const PATH_MAX: usize = 4096;
const NAME_MAX: usize = 255;
static NAMEI_MUTATION_LOCK: Mutex<()> = Mutex::new(());

fn task_root_path() -> Arc<Path> {
    current_task()
        .map(|task| task.root())
        .unwrap_or_else(root_path)
}

/// 路径名解析状态量
pub struct Nameidata {
    pub mnt: Arc<VfsMount>,
    pub dentry: Arc<Dentry>,
    path_segments: Vec<String>,
    depth: usize,
    requires_directory: bool,
}

impl Nameidata {
    pub fn new(file_name: &str) -> Self {
        let task = current_task().expect("[kernel] current task is None.");
        Self::new_from_path(file_name, task.cwd())
    }

    pub fn new_at(dirfd: isize, file_name: &str) -> SysResult<Self> {
        validate_path_components(file_name)?;
        let base = base_path_from_dirfd(dirfd, file_name)?;
        Ok(Self::new_from_path(file_name, base))
    }

    fn new_from_path(file_name: &str, base: Arc<Path>) -> Self {
        let path_segments: Vec<String> = file_name
            .split('/')
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();

        let base = if file_name.starts_with("/") {
            task_root_path()
        } else {
            base
        };

        Nameidata {
            mnt: base.mnt.clone(),
            dentry: base.dentry.clone(),
            path_segments,
            depth: 0,
            requires_directory: file_name.ends_with('/'),
        }
    }
}

fn validate_path_components(path: &str) -> SysResult {
    if path.split('/').any(|name| name.len() > NAME_MAX) {
        return Err(Errno::ENAMETOOLONG);
    }
    Ok(())
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
    let name = nd.path_segments[nd.depth].as_str();
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
    let child_inode = current_dir_inode.lookup(&nd.dentry.current_abs_path(), name)?;

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
    let root = task_root_path();
    if Arc::ptr_eq(&nd.mnt, &root.mnt) && Arc::ptr_eq(&nd.dentry, &root.dentry) {
        return;
    }
    if Arc::ptr_eq(&nd.dentry, &nd.mnt.root) {
        if let Some(mount) = get_mount_by_vfsmount(&nd.mnt) {
            if let Some(parent) = mount.parent.as_ref().and_then(|parent| parent.upgrade()) {
                if Arc::ptr_eq(&parent.vfs_mount, &root.mnt)
                    && Arc::ptr_eq(&mount.mountpoint, &root.dentry)
                {
                    return;
                }
                nd.mnt = parent.vfs_mount.clone();
                nd.dentry = mount.mountpoint.get_parent_or_self();
                return;
            }
        }
    }
    nd.dentry = nd.dentry.get_parent_or_self();
}

fn child_abs_path(parent: &Arc<Dentry>, name: &str) -> alloc::string::String {
    let parent_path = parent.current_abs_path();
    if parent_path == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent_path, name)
    }
}

fn invalidate_directory_metadata(dentry: &Arc<Dentry>) {
    if let Some(inode) = dentry.get_inode().as_any().downcast_ref::<Ext4Inode>() {
        inode.namespace_changed();
    }
}

fn dentry_name(dentry: &Arc<Dentry>) -> SysResult<String> {
    dentry
        .current_abs_path()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(String::from)
        .ok_or(Errno::EBUSY)
}

fn inode_allows_perm(dentry: &Arc<Dentry>, mask: u32) -> SysResult<bool> {
    let task = current_task().expect("[kernel] current task is None.");
    let euid = task.fsuid() as u32;
    if euid == 0 {
        return Ok(true);
    }

    let stat = dentry.get_inode().stat(&dentry.current_abs_path())?;
    let mode = stat.mode & 0o777;
    let perm = if euid == stat.uid {
        (mode >> 6) & 0o7
    } else if task.in_group(stat.gid as usize) {
        (mode >> 3) & 0o7
    } else {
        mode & 0o7
    };
    Ok(perm & mask != 0)
}

fn inode_allows_write(dentry: &Arc<Dentry>) -> SysResult<bool> {
    inode_allows_perm(dentry, 0o2)
}

fn inode_allows_read(dentry: &Arc<Dentry>) -> SysResult<bool> {
    inode_allows_perm(dentry, 0o4)
}

fn check_dir_write_permission(dentry: &Arc<Dentry>) -> SysResult {
    if inode_allows_write(dentry)? {
        Ok(())
    } else {
        Err(Errno::EACCES)
    }
}

fn check_dir_write_and_search_permission(dentry: &Arc<Dentry>) -> SysResult {
    check_dir_search_permission(dentry)?;
    check_dir_write_permission(dentry)
}

fn check_sticky_rename_permission(parent: &Arc<Dentry>, target: &Arc<Dentry>) -> SysResult {
    let parent_stat = parent.get_inode().stat(&parent.current_abs_path())?;
    if parent_stat.mode & 0o1000 == 0 {
        return Ok(());
    }

    let task = current_task().expect("[kernel] current task is None.");
    let euid = task.fsuid() as u32;
    if euid == 0 || euid == parent_stat.uid {
        return Ok(());
    }

    let target_stat = target.get_inode().stat(&target.current_abs_path())?;
    if euid == target_stat.uid {
        Ok(())
    } else {
        Err(Errno::EPERM)
    }
}

pub fn check_dir_search_permission(dentry: &Arc<Dentry>) -> SysResult {
    if inode_allows_perm(dentry, 0o1)? {
        Ok(())
    } else {
        Err(Errno::EACCES)
    }
}

fn check_open_permission(dentry: &Arc<Dentry>, flags: OpenFlags) -> SysResult {
    let inode = dentry.get_inode();
    let ty = inode.node_type();
    if ty == InodeType::Directory && flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR) {
        return Err(Errno::EISDIR);
    }

    // O_PATH only acquires a path reference.  Linux does not apply the
    // target inode's read/write permission checks to this kind of open.
    if flags.contains(OpenFlags::O_PATH) {
        return Ok(());
    }

    if flags.contains(OpenFlags::O_NOATIME) {
        let task = current_task().expect("[kernel] current task is None.");
        let stat = inode.stat(&dentry.current_abs_path())?;
        let fsuid = task.fsuid() as u32;
        if fsuid != 0 && fsuid != stat.uid {
            return Err(Errno::EPERM);
        }
    }

    if flags.contains(OpenFlags::O_RDWR) {
        if !inode_allows_read(dentry)? || !inode_allows_write(dentry)? {
            return Err(Errno::EACCES);
        }
    } else if flags.contains(OpenFlags::O_WRONLY) {
        if !inode_allows_write(dentry)? {
            return Err(Errno::EACCES);
        }
    } else if !inode_allows_read(dentry)? {
        return Err(Errno::EACCES);
    }

    Ok(())
}

fn check_mount_writable(mnt: &Arc<VfsMount>) -> SysResult {
    if mnt.is_readonly() {
        Err(Errno::EROFS)
    } else {
        Ok(())
    }
}

fn check_mount_device_allowed(
    mnt: &Arc<VfsMount>,
    inode: &Arc<dyn super::vfs::InodeOp>,
) -> SysResult {
    if mnt.has_flag(MS_NODEV)
        && matches!(
            inode.node_type(),
            InodeType::CharDevice | InodeType::BlockDevice
        )
    {
        Err(Errno::EACCES)
    } else {
        Ok(())
    }
}

fn created_owner(parent: &Arc<Dentry>) -> SysResult<(u32, u32)> {
    let task = current_task().expect("[kernel] current task is None.");
    let parent_stat = parent.get_inode().stat(&parent.current_abs_path())?;
    let gid = if parent_stat.mode & 0o2000 != 0 {
        parent_stat.gid
    } else {
        task.fsgid() as u32
    };
    Ok((task.fsuid() as u32, gid))
}

fn created_mode(parent: &Arc<Dentry>, requested_mode: usize, ty: InodeType) -> SysResult<u32> {
    let task = current_task().expect("[kernel] current task is None.");
    let parent_stat = parent.get_inode().stat(&parent.current_abs_path())?;
    let mut mode = (requested_mode & 0o7777) as u32;
    mode &= !(task.umask() as u32);
    if ty == InodeType::Directory && parent_stat.mode & 0o2000 != 0 {
        mode |= 0o2000;
    }
    if ty == InodeType::Regular
        && parent_stat.mode & 0o2000 != 0
        && task.fsuid() != 0
        && !task.in_group(parent_stat.gid as usize)
    {
        mode &= !0o2000;
    }
    Ok(mode)
}

fn normalized_open_flags(flags: OpenFlags) -> OpenFlags {
    if flags.contains(OpenFlags::O_PATH) {
        flags
            & (OpenFlags::O_PATH
                | OpenFlags::O_DIRECTORY
                | OpenFlags::O_NOFOLLOW
                | OpenFlags::O_CLOEXEC)
    } else {
        flags
    }
}

fn require_directory(path: &Arc<Path>, required: bool) -> SysResult {
    if required && path.dentry.get_inode().node_type() != InodeType::Directory {
        Err(Errno::ENOTDIR)
    } else {
        Ok(())
    }
}

fn reject_trailing_non_directory_creation(nd: &mut Nameidata) -> SysResult {
    if !nd.requires_directory {
        return Ok(());
    }
    match lookup_dentry(nd) {
        Ok(_) => Err(Errno::EEXIST),
        Err(Errno::ENOENT) => Err(Errno::ENOENT),
        Err(err) => Err(err),
    }
}

fn tmpfile_meta(parent: &Arc<Dentry>, mode: usize) -> SysResult<TmpFileMeta> {
    let (uid, gid) = created_owner(parent)?;
    Ok(TmpFileMeta {
        mode: created_mode(parent, mode, InodeType::Regular)?,
        uid,
        gid,
    })
}

fn install_created_inode(
    parent: &Arc<Dentry>,
    name: &str,
    inode: Arc<dyn super::vfs::InodeOp>,
    mode: u32,
    uid: u32,
    gid: u32,
) -> SysResult<Arc<Dentry>> {
    let child_path = child_abs_path(parent, name);
    let child = Arc::new(Dentry::new(
        child_path.clone(),
        Some(parent.clone()),
        inode.clone(),
    ));
    let configure_result = match inode.set_owner_and_mode(&child_path, uid, gid, Some(mode)) {
        Err(Errno::EOPNOTSUPP) => inode
            .set_mode(&child_path, mode)
            .and_then(|_| inode.set_owner(&child_path, uid, gid)),
        result => result,
    };
    if let Err(err) = configure_result {
        // The lower create has already allocated a namespace entry.  Keep it
        // unpublished until all metadata is committed, and roll it back on
        // failure so callers never observe a successful-looking partial file.
        let _ = parent.get_inode().unlink(&child);
        return Err(err);
    }
    parent.insert_child(name, child.clone());
    insert_dentry_cache(child.clone());
    Ok(child)
}

fn install_created_symlink(
    parent: &Arc<Dentry>,
    name: &str,
    inode: Arc<dyn super::vfs::InodeOp>,
    uid: u32,
    gid: u32,
) -> SysResult<Arc<Dentry>> {
    let child_path = child_abs_path(parent, name);
    let child = Arc::new(Dentry::new(
        child_path.clone(),
        Some(parent.clone()),
        inode.clone(),
    ));
    if let Err(err) = inode.set_owner(&child_path, uid, gid) {
        let _ = parent.get_inode().unlink(&child);
        return Err(err);
    }
    parent.insert_child(name, child.clone());
    insert_dentry_cache(child.clone());
    Ok(child)
}

pub fn open_last_lookups(nd: &mut Nameidata, flags: usize, mode: usize) -> SysResult<Arc<File>> {
    let flags = normalized_open_flags(OpenFlags::from(flags));

    // 目标为根目录或工作目录
    if nd.path_segments.is_empty() {
        let path = Path::new(nd.mnt.clone(), nd.dentry.clone());
        let inode = nd.dentry.get_inode();
        if flags.contains(OpenFlags::O_TMPFILE) {
            if inode.node_type() != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }
            let meta = tmpfile_meta(&nd.dentry, mode)?;
            return Ok(Arc::new(File::new_tmpfile(path, inode, flags, meta)));
        }
        if flags.contains(OpenFlags::O_CREATE) {
            return Err(Errno::EISDIR);
        }
        check_open_permission(&nd.dentry, flags)?;
        return Ok(Arc::new(File::new(path, inode, flags)));
    }

    let name = nd.path_segments[nd.depth].clone();

    let dentry = if name == "." {
        nd.dentry.clone()
    } else if name == ".." {
        follow_dotdot(nd);
        nd.dentry.clone()
    } else {
        match lookup_dentry(nd) {
            // 成功
            Ok(dentry) => {
                let mut dentry = dentry;
                let mut inode = dentry.get_inode();
                if flags.contains(OpenFlags::O_TMPFILE) {
                    if inode.node_type() != InodeType::Directory {
                        return Err(Errno::ENOTDIR);
                    }
                    let meta = tmpfile_meta(&dentry, mode)?;
                    return Ok(Arc::new(File::new_tmpfile(
                        Path::new(nd.mnt.clone(), dentry),
                        inode,
                        flags,
                        meta,
                    )));
                }
                if flags.contains(OpenFlags::O_CREATE | OpenFlags::O_EXCL) {
                    return Err(Errno::EEXIST);
                }
                if flags.contains(OpenFlags::O_NOFOLLOW)
                    && inode.node_type() == InodeType::SymLink
                    && !flags.contains(OpenFlags::O_PATH)
                {
                    return Err(Errno::ELOOP);
                }
                if flags.contains(OpenFlags::O_CREATE) && inode.node_type() == InodeType::SymLink {
                    let symlink_base = Path::new(nd.mnt.clone(), nd.dentry.clone());
                    let target = inode.read_link(&dentry.current_abs_path())?;
                    let target = if nd.requires_directory && !target.ends_with('/') {
                        format!("{}/", target)
                    } else {
                        target
                    };
                    match resolve_path_from(symlink_base.clone(), &target, true, true, true, 1) {
                        Ok(target_path) => {
                            nd.mnt = target_path.mnt.clone();
                            dentry = target_path.dentry.clone();
                            inode = dentry.get_inode();
                        }
                        Err(Errno::ENOENT) => {
                            return path_open_from_base(
                                symlink_base,
                                &target,
                                flags.bits() as usize,
                                mode,
                            );
                        }
                        Err(err) => return Err(err),
                    }
                }
                if flags.contains(OpenFlags::O_CREATE) && inode.node_type() == InodeType::Directory
                {
                    return Err(Errno::EISDIR);
                }
                if nd.requires_directory && inode.node_type() != InodeType::Directory {
                    return Err(Errno::ENOTDIR);
                }
                // 期望打开目录，但实际文件类型不是目录，返回错误
                if flags.contains(OpenFlags::O_DIRECTORY)
                    && inode.node_type() != InodeType::Directory
                {
                    return Err(Errno::ENOTDIR);
                }
                check_mount_device_allowed(&nd.mnt, &inode)?;
                if flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR | OpenFlags::O_TRUNC) {
                    check_mount_writable(&nd.mnt)?;
                }
                check_open_permission(&dentry, flags)?;
                // TODO: 此处默认 dentry 不是负目录项，在引入缓存后需修改
                dentry
            }
            Err(Errno::ENOENT) if flags.contains(OpenFlags::O_CREATE) => {
                let mutation_guard = NAMEI_MUTATION_LOCK.lock();
                match lookup_dentry(nd) {
                    Ok(_) => {
                        drop(mutation_guard);
                        return open_last_lookups(nd, flags.bits() as usize, mode);
                    }
                    Err(Errno::ENOENT) => {}
                    Err(err) => return Err(err),
                }
                // 期望打开目录，但目标不存在
                if flags.contains(OpenFlags::O_DIRECTORY) {
                    return Err(Errno::ENOTDIR);
                }
                if nd.requires_directory {
                    return Err(Errno::EISDIR);
                }
                check_mount_writable(&nd.mnt)?;
                check_dir_write_and_search_permission(&nd.dentry)?;
                let current_dir_inode = nd.dentry.get_inode();
                let create_mode = created_mode(&nd.dentry, mode, InodeType::Regular)?;
                let (uid, gid) = created_owner(&nd.dentry)?;
                let inode = current_dir_inode.create(
                    &nd.dentry.current_abs_path(),
                    name.as_str(),
                    InodeType::Regular,
                )?;
                install_created_inode(&nd.dentry, name.as_str(), inode, create_mode, uid, gid)?
            }
            Err(err) => return Err(err),
        }
    };

    let inode = dentry.get_inode();
    let file = Arc::new(File::new(Path::new(nd.mnt.clone(), dentry), inode, flags));
    if flags.contains(OpenFlags::O_TRUNC)
        && flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
    {
        file.truncate(0)?;
    }
    Ok(file)
}

/// 根据路径打开文件
pub fn path_open(dirfd: isize, path: &str, flags: usize, mode: usize) -> SysResult<Arc<File>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let open_flags = normalized_open_flags(OpenFlags::from(flags));
    if !open_flags.contains(OpenFlags::O_CREATE) {
        let path = if open_flags.contains(OpenFlags::O_NOFOLLOW) {
            filename_lookup_no_follow_final_symlink(dirfd, path)?
        } else {
            filename_lookup(dirfd, path, 0)?
        };
        let inode = path.dentry.get_inode();
        if open_flags.contains(OpenFlags::O_NOFOLLOW)
            && inode.node_type() == InodeType::SymLink
            && !open_flags.contains(OpenFlags::O_PATH)
        {
            return Err(Errno::ELOOP);
        }
        if open_flags.contains(OpenFlags::O_TMPFILE) {
            if inode.node_type() != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }
            let meta = tmpfile_meta(&path.dentry, mode)?;
            return Ok(Arc::new(File::new_tmpfile(path, inode, open_flags, meta)));
        }
        if open_flags.contains(OpenFlags::O_DIRECTORY) && inode.node_type() != InodeType::Directory
        {
            return Err(Errno::ENOTDIR);
        }
        check_mount_device_allowed(&path.mnt, &inode)?;
        if open_flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR | OpenFlags::O_TRUNC) {
            check_mount_writable(&path.mnt)?;
        }
        check_open_permission(&path.dentry, open_flags)?;
        let file = Arc::new(File::new(path, inode, open_flags));
        if open_flags.contains(OpenFlags::O_TRUNC)
            && open_flags.intersects(OpenFlags::O_WRONLY | OpenFlags::O_RDWR)
        {
            file.truncate(0)?;
        }
        return Ok(file);
    }

    // O_CREAT 路径沿用 parent-walk 流程；最终 symlink 的 O_NOFOLLOW 语义在 open_last_lookups 处理。
    let base = base_path_from_dirfd(dirfd, path)?;
    path_open_from_base(base, path, flags, mode)
}

fn path_open_from_base(
    base: Arc<Path>,
    path: &str,
    flags: usize,
    mode: usize,
) -> SysResult<Arc<File>> {
    validate_path_components(path)?;
    let mut nd = Nameidata::new_from_path(path, base);
    link_path_walk(&mut nd)?;
    open_last_lookups(&mut nd, flags, mode)
}

/// 根据路径创建文件
pub fn filename_create(dirfd: isize, path: &str, ty: InodeType, mode: usize) -> SysResult {
    filename_create_impl(dirfd, path, ty, mode, None)
}

/// Create a special inode while preserving its device payload through namei.
pub fn filename_mknod(dirfd: isize, path: &str, ty: InodeType, mode: usize, dev: u32) -> SysResult {
    filename_create_impl(dirfd, path, ty, mode, Some(dev))
}

fn filename_create_impl(
    dirfd: isize,
    path: &str,
    ty: InodeType,
    mode: usize,
    special_dev: Option<u32>,
) -> SysResult {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }

    let _mutation_guard = NAMEI_MUTATION_LOCK.lock();
    let mut nd = Nameidata::new_at(dirfd, path)?;
    link_path_walk(&mut nd)?;

    if ty != InodeType::Directory {
        reject_trailing_non_directory_creation(&mut nd)?;
    }

    // 目标为根目录或工作目录，返回错误
    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth].clone();
    if name == "." || name == ".." {
        return Err(Errno::EEXIST);
    }
    check_mount_writable(&nd.mnt)?;
    check_dir_write_and_search_permission(&nd.dentry)?;

    // TODO: 引入负目录项需进行修改，这里先做简单实现
    match lookup_dentry(&mut nd) {
        Ok(_) => Err(Errno::EEXIST),
        // 未找到目标文件，创建文件
        Err(Errno::ENOENT) => {
            let current_dir_inode = nd.dentry.get_inode();
            let create_mode = created_mode(&nd.dentry, mode, ty)?;
            let (uid, gid) = created_owner(&nd.dentry)?;
            let inode = if let Some(dev) = special_dev {
                current_dir_inode.create_special(
                    &nd.dentry.current_abs_path(),
                    name.as_str(),
                    ty,
                    dev,
                )?
            } else {
                current_dir_inode.create(&nd.dentry.current_abs_path(), name.as_str(), ty)?
            };
            install_created_inode(&nd.dentry, name.as_str(), inode, create_mode, uid, gid)?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

pub fn filename_symlink(dirfd: isize, target: &str, newpath: &str) -> SysResult {
    if target.is_empty() || newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    let _mutation_guard = NAMEI_MUTATION_LOCK.lock();
    // symlinkat 的 linkpath 需要按“父目录 + 新名字”解析：
    // 中间路径仍正常解析，最后一级必须不存在。
    let mut nd = Nameidata::new_at(dirfd, newpath)?;
    link_path_walk(&mut nd)?;

    reject_trailing_non_directory_creation(&mut nd)?;

    // 不能把根目录或当前工作目录本身替换成一个新的符号链接目录项。
    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth].clone();
    // "." 和 ".." 不是普通文件名，不能作为新符号链接的名字。
    if name == "." || name == ".." {
        return Err(Errno::EEXIST);
    }
    check_mount_writable(&nd.mnt)?;
    check_dir_write_and_search_permission(&nd.dentry)?;

    match lookup_dentry(&mut nd) {
        Ok(_) => Err(Errno::EEXIST),
        Err(Errno::ENOENT) => {
            let parent_inode = nd.dentry.get_inode();
            let (uid, gid) = created_owner(&nd.dentry)?;
            // 目标字符串原样写入 symlink inode；相对路径到真正解析时再以链接所在目录为基准解释。
            let inode =
                parent_inode.symlink(target, &nd.dentry.current_abs_path(), name.as_str())?;
            install_created_symlink(&nd.dentry, name.as_str(), inode, uid, gid)?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// 根据路径查询文件
pub fn filename_lookup(dirfd: isize, path: &str, _mode: usize) -> SysResult<Arc<Path>> {
    // 默认 lookup 语义和 Linux 一致：跟随最后一级符号链接，也允许穿过最终挂载点。
    resolve_path(dirfd, path, true, true, true)
}

pub fn filename_lookup_no_follow_final_mount(dirfd: isize, path: &str) -> SysResult<Arc<Path>> {
    // umount 等场景需要定位挂载点本身，因此最后一级不能穿过 mount。
    resolve_path(dirfd, path, true, false, true)
}

pub fn filename_lookup_no_follow_final_symlink(dirfd: isize, path: &str) -> SysResult<Arc<Path>> {
    // readlinkat/lstat 需要拿到 symlink inode 自身；中间路径中的 symlink 仍要正常跟随。
    resolve_path(dirfd, path, false, true, true)
}

fn filename_lookup_no_follow_final_component(dirfd: isize, path: &str) -> SysResult<Arc<Path>> {
    resolve_path(dirfd, path, false, true, false)
}

fn resolve_path(
    dirfd: isize,
    path: &str,
    follow_final_symlink: bool,
    follow_final_mount: bool,
    follow_trailing_symlink: bool,
) -> SysResult<Arc<Path>> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    validate_path_components(path)?;
    // dirfd 只决定相对路径的起点；绝对路径会在 Nameidata::new_from_path 中切回 root_path。
    let base = base_path_from_dirfd(dirfd, path)?;
    match resolve_path_from(
        base,
        path,
        follow_final_symlink,
        follow_final_mount,
        follow_trailing_symlink,
        0,
    ) {
        Err(Errno::ENOENT) => {
            if let Some(alias) = glibc_default_lib_alias(path) {
                resolve_path_from(
                    task_root_path(),
                    &alias,
                    follow_final_symlink,
                    follow_final_mount,
                    follow_trailing_symlink,
                    0,
                )
            } else {
                Err(Errno::ENOENT)
            }
        }
        result => result,
    }
}

fn glibc_default_lib_alias(path: &str) -> Option<String> {
    for prefix in ["/lib/", "/lib64/", "/usr/lib/", "/usr/lib64/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return Some(format!("/glibc/lib/{}", rest));
        }
    }
    None
}

fn resolve_path_from(
    base: Arc<Path>,
    path: &str,
    follow_final_symlink: bool,
    follow_final_mount: bool,
    follow_trailing_symlink: bool,
    symlink_follows: usize,
) -> SysResult<Arc<Path>> {
    // 限制 symlink 展开次数，防止 a -> b -> a 这种循环把内核拖进无限递归。
    if symlink_follows >= MAX_SYMLINK_FOLLOWS {
        return Err(Errno::ELOOP);
    }

    let mut nd = Nameidata::new_from_path(path, base);
    // 空 segment 表示路径就是起点本身，例如 "/" 或相对路径中的空字符串。
    if nd.path_segments.is_empty() {
        let resolved = Path::new(nd.mnt.clone(), nd.dentry.clone());
        require_directory(&resolved, nd.requires_directory)?;
        return Ok(resolved);
    }

    while nd.depth < nd.path_segments.len() {
        let name = nd.path_segments[nd.depth].as_str();
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
        if nd.dentry.get_inode().node_type() != InodeType::Directory {
            return Err(Errno::ENOTDIR);
        }
        check_dir_search_permission(&nd.dentry)?;
        // 中间路径必须穿过 mount；最后一级是否穿过 mount 由调用者决定。
        let child = lookup_dentry_maybe_follow_mount(&mut nd, !is_last || follow_final_mount)?;
        let child_path = Path::new(nd.mnt.clone(), child.clone());
        let inode = child.get_inode();

        // 中间 symlink 一定要展开；最后一级是否展开由 follow_final_symlink 决定。
        if inode.node_type() == InodeType::SymLink
            && (!is_last
                || follow_final_symlink
                || (nd.requires_directory && follow_trailing_symlink))
        {
            if child_path.mnt.has_flag(MS_NOSYMFOLLOW) {
                return Err(Errno::ELOOP);
            }
            let target = inode.read_link(&child_path.abs_path())?;
            // target 替换当前 symlink，其余未解析 segment 继续拼到 target 后面。
            let next_path = join_symlink_target(
                &target,
                &nd.path_segments[nd.depth + 1..],
                nd.requires_directory,
            );
            if next_path.len() > PATH_MAX {
                return Err(Errno::ENAMETOOLONG);
            }
            validate_path_components(&next_path)?;
            let next_base = if target.starts_with('/') {
                // 绝对 symlink 目标从当前进程 root 开始解析。
                task_root_path()
            } else {
                // 相对 symlink 目标从 symlink 所在目录开始解析。
                symlink_base
            };
            return resolve_path_from(
                next_base,
                &next_path,
                follow_final_symlink,
                follow_final_mount,
                follow_trailing_symlink,
                symlink_follows + 1,
            );
        }

        if is_last {
            // 最后一级不是需要展开的 symlink，当前 child 就是解析结果。
            require_directory(&child_path, nd.requires_directory)?;
            return Ok(child_path);
        }

        nd.dentry = child;
        nd.depth += 1;
    }

    Ok(Path::new(nd.mnt.clone(), nd.dentry.clone()))
}

fn join_symlink_target(target: &str, rest: &[String], requires_directory: bool) -> String {
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
    if requires_directory && !path.ends_with('/') {
        path.push('/');
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

    let _mutation_guard = NAMEI_MUTATION_LOCK.lock();
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
    let name = nd.path_segments[nd.depth].clone();
    if name == "." || name == ".." {
        return if remove_dir {
            Err(Errno::EINVAL)
        } else {
            Err(Errno::EISDIR)
        };
    }
    check_mount_writable(&nd.mnt)?;
    check_dir_write_and_search_permission(&nd.dentry)?;

    let target = lookup_dentry(&mut nd)?;
    let target_ty = target.get_inode().node_type();
    if nd.requires_directory && target_ty != InodeType::Directory {
        return Err(Errno::ENOTDIR);
    }
    if target_ty == InodeType::Directory && !remove_dir {
        return Err(Errno::EISDIR);
    }
    if target_ty != InodeType::Directory && remove_dir {
        return Err(Errno::ENOTDIR);
    }

    let parent = target.get_parent().ok_or(Errno::EBUSY)?;
    check_sticky_rename_permission(&parent, &target)?;
    let name = dentry_name(&target)?;
    let target_path = target.current_abs_path();
    parent.get_inode().unlink(&target)?;
    parent.remove_child(name.as_str());
    remove_dentry_cache_tree(&target_path);
    Ok(())
}

pub fn filename_link_tmpfile(file: &File, newdirfd: isize, newpath: &str) -> SysResult {
    if newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    let _mutation_guard = NAMEI_MUTATION_LOCK.lock();
    let meta = file.tmpfile_meta().ok_or(Errno::EINVAL)?;

    let mut nd = Nameidata::new_at(newdirfd, newpath)?;
    link_path_walk(&mut nd)?;

    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth].clone();
    if name == "." || name == ".." {
        return Err(Errno::EEXIST);
    }
    check_mount_writable(&nd.mnt)?;
    check_dir_write_and_search_permission(&nd.dentry)?;

    match lookup_dentry(&mut nd) {
        Ok(_) => Err(Errno::EEXIST),
        Err(Errno::ENOENT) => {
            let parent_inode = nd.dentry.get_inode();
            let inode = parent_inode.create(
                &nd.dentry.current_abs_path(),
                name.as_str(),
                InodeType::Regular,
            )?;
            let child_path = child_abs_path(&nd.dentry, name.as_str());
            let child = Arc::new(Dentry::new(
                child_path.clone(),
                Some(nd.dentry.clone()),
                inode.clone(),
            ));
            let materialize_result = (|| -> SysResult {
                inode.set_owner_and_mode(&child_path, meta.uid, meta.gid, Some(meta.mode))?;
                let data = file.read_all()?;
                if let Some(page_cache) = inode.get_page_cache() {
                    let mut offset = 0usize;
                    while offset < data.len() {
                        let written = page_cache.write_at(offset, &data[offset..], None)?;
                        if written == 0 {
                            return Err(Errno::EIO);
                        }
                        offset += written;
                    }
                    if !data.is_empty() {
                        let ms = get_time_ms();
                        let now = TimeSpec {
                            sec: (ms / 1000) as isize,
                            nsec: ((ms % 1000) * 1_000_000) as isize,
                        };
                        inode.note_data_write(child_path.as_str(), now)?;
                        register_dirty_owner(
                            page_cache,
                            inode.clone(),
                            nd.mnt.fs.clone(),
                            child_path.as_str(),
                        );
                    }
                } else {
                    let mut offset = 0usize;
                    while offset < data.len() {
                        let written =
                            inode.write_at(child_path.as_str(), offset, &data[offset..])?;
                        if written == 0 {
                            return Err(Errno::EIO);
                        }
                        offset += written;
                    }
                }
                Ok(())
            })();
            if let Err(err) = materialize_result {
                let _ = parent_inode.unlink(&child);
                return Err(err);
            }
            nd.dentry.insert_child(name.as_str(), child.clone());
            insert_dentry_cache(child);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// 根据两个路径创建硬链接。
pub fn filename_link(
    olddirfd: isize,
    oldpath: &str,
    newdirfd: isize,
    newpath: &str,
    follow_old_symlink: bool,
) -> SysResult {
    if oldpath.is_empty() || newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    let _mutation_guard = NAMEI_MUTATION_LOCK.lock();
    let old_path = if follow_old_symlink {
        filename_lookup(olddirfd, oldpath, 0)?
    } else {
        filename_lookup_no_follow_final_symlink(olddirfd, oldpath)?
    };

    let mut nd = Nameidata::new_at(newdirfd, newpath)?;
    link_path_walk(&mut nd)?;

    reject_trailing_non_directory_creation(&mut nd)?;

    // 目标为根目录或工作目录
    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth].clone();
    if name == "." || name == ".." {
        return Err(Errno::EEXIST);
    }
    check_mount_writable(&nd.mnt)?;
    check_dir_write_and_search_permission(&nd.dentry)?;

    if old_path.dentry.get_inode().node_type() == InodeType::Directory {
        return Err(Errno::EPERM);
    }

    let old_stat = old_path
        .dentry
        .get_inode()
        .stat(old_path.abs_path().as_str())?;
    let parent_stat = nd.dentry.get_inode().stat(&nd.dentry.current_abs_path())?;
    if old_stat.dev != parent_stat.dev {
        return Err(Errno::EXDEV);
    }

    match lookup_dentry(&mut nd) {
        Ok(_) => Err(Errno::EEXIST),
        Err(Errno::ENOENT) => {
            let bare_dentry = Dentry::negative(
                child_abs_path(&nd.dentry, name.as_str()),
                Some(nd.dentry.clone()),
            );
            let old_inode = old_path.dentry.get_inode();
            old_inode.link(&old_path.abs_path(), bare_dentry.clone())?;
            bare_dentry.inner.lock().inode = Some(old_inode);
            nd.dentry.insert_child(name.as_str(), bare_dentry.clone());
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
    no_replace: bool,
) -> SysResult {
    if oldpath.is_empty() || newpath.is_empty() {
        return Err(Errno::ENOENT);
    }

    let _mutation_guard = NAMEI_MUTATION_LOCK.lock();
    let old = filename_lookup_no_follow_final_component(olddirfd, oldpath)?;
    if oldpath.ends_with('/') && old.dentry.get_inode().node_type() != InodeType::Directory {
        return Err(Errno::ENOTDIR);
    }
    let old_parent = old
        .dentry
        .get_parent()
        .unwrap_or_else(|| old.dentry.clone());
    check_mount_writable(&old.mnt)?;
    check_dir_write_and_search_permission(&old_parent)?;
    check_sticky_rename_permission(&old_parent, &old.dentry)?;

    let mut nd = Nameidata::new_at(newdirfd, newpath)?;
    link_path_walk(&mut nd)?;

    if nd.path_segments.is_empty() {
        return Err(Errno::EEXIST);
    }

    let name = nd.path_segments[nd.depth].clone();
    if name == "." || name == ".." {
        return Err(Errno::EBUSY);
    }
    check_mount_writable(&nd.mnt)?;
    check_dir_write_and_search_permission(&nd.dentry)?;

    let new_abs = child_abs_path(&nd.dentry, name.as_str());
    if new_abs == old.abs_path() {
        return if no_replace {
            Err(Errno::EEXIST)
        } else {
            Ok(())
        };
    }

    let old_ty = old.dentry.get_inode().node_type();
    if nd.requires_directory && old_ty != InodeType::Directory {
        return Err(Errno::ENOTDIR);
    }
    if old_ty == InodeType::Directory && new_abs.starts_with(&(old.abs_path() + "/")) {
        return Err(Errno::EINVAL);
    }
    // 防止 ext4_frename 在目标为非空目录时产生嵌套语义，损坏文件系统。
    let mut replaced_target: Option<(Arc<dyn super::vfs::InodeOp>, String, InodeType, bool)> = None;
    if let Ok(existing) = lookup_dentry(&mut nd) {
        if no_replace {
            return Err(Errno::EEXIST);
        }
        let existing_path = existing.current_abs_path();
        let existing_ty = existing.get_inode().node_type();
        if old_ty != InodeType::Directory && existing_ty == InodeType::Directory {
            return Err(Errno::EISDIR);
        }
        if old_ty == InodeType::Directory && existing_ty != InodeType::Directory {
            return Err(Errno::ENOTDIR);
        }

        let old_stat = old.dentry.get_inode().stat(&old.abs_path())?;
        let existing_stat = existing.get_inode().stat(&existing_path)?;
        if old_stat.dev == existing_stat.dev && old_stat.ino == existing_stat.ino {
            return Ok(());
        }
        check_sticky_rename_permission(&nd.dentry, &existing)?;

        if existing_ty == InodeType::Directory {
            let entries = existing.get_inode().readdir(&existing_path)?;
            let has_content = entries
                .iter()
                .any(|e| e.d_name != b".\0" && e.d_name != b"..\0");
            if has_content {
                return Err(Errno::ENOTEMPTY);
            }
        }
        let existing_inode = existing.get_inode();
        existing_inode
            .as_any()
            .downcast_ref::<Ext4Inode>()
            .ok_or(Errno::EXDEV)?;
        let existing_stat = existing_inode.stat(&existing_path)?;
        let deferred = existing_ty == InodeType::Directory || existing_stat.nlink <= 1;
        let backup_path = alloc::format!(
            "{}.respos_rename_backup_{}",
            existing_path,
            existing_stat.ino
        );
        Ext4Inode::file_rename(&existing_path, &backup_path)?;
        replaced_target = Some((existing_inode, backup_path, existing_ty, deferred));
        nd.dentry.remove_child(name.as_str());
        remove_dentry_cache_tree(&existing_path);
    }

    if let Err(err) = Ext4Inode::file_rename(&old.abs_path(), &new_abs) {
        if let Some((_, backup_path, _, _)) = replaced_target.as_ref() {
            let _ = Ext4Inode::file_rename(backup_path, &new_abs);
        }
        return Err(err);
    }

    invalidate_directory_metadata(&old_parent);
    if !Arc::ptr_eq(&old_parent, &nd.dentry) {
        invalidate_directory_metadata(&nd.dentry);
    }

    if let Some((inode, backup_path, ty, deferred)) = replaced_target {
        // 主 rename 已提交，清理暂存目录失败不能再向用户报告 rename 失败，
        // 否则会造成“返回错误但命名空间已改变”的 ABI。残留备份可在后续维护清理。
        if Ext4Inode::remove_path(&backup_path, ty, deferred).is_ok() {
            if let Some(inode) = inode.as_any().downcast_ref::<Ext4Inode>() {
                inode.invalidate_raw_metadata();
                if deferred {
                    inode.mark_unlinked();
                }
            }
        }
    }

    // 保持 dentry identity：已打开 File、cwd 以及后代 Path 都应观察到新父链。
    old_parent.remove_child(dentry_name(&old.dentry)?.as_str());

    remove_dentry_cache_tree(&old.abs_path().as_str());
    remove_dentry_cache_tree(&new_abs);
    old.dentry.relocate(new_abs, nd.dentry.clone());
    nd.dentry.insert_child(name.as_str(), old.dentry.clone());
    insert_dentry_cache(old.dentry.clone());

    Ok(())
}

/// 路径解析主函数，循环解析每一层，定位到最后的目标
pub fn link_path_walk(nd: &mut Nameidata) -> SysResult {
    let mut symlink_follows = 0usize;

    'restart: loop {
        if nd.path_segments.is_empty() {
            return Ok(());
        }

        let len = nd.path_segments.len() - 1;
        while nd.depth < len {
            let name = nd.path_segments[nd.depth].as_str();
            if name == "." {
                // do nothing
            } else if name == ".." {
                follow_dotdot(nd);
            } else {
                let symlink_base = Path::new(nd.mnt.clone(), nd.dentry.clone());
                if nd.dentry.get_inode().node_type() == InodeType::Directory {
                    check_dir_search_permission(&nd.dentry)?;
                }
                let child_dentry = lookup_dentry(nd)?;
                let child_path = Path::new(nd.mnt.clone(), child_dentry.clone());
                let inode = child_dentry.get_inode();
                if inode.node_type() == InodeType::SymLink {
                    if child_path.mnt.has_flag(MS_NOSYMFOLLOW) {
                        return Err(Errno::ELOOP);
                    }
                    if symlink_follows >= MAX_SYMLINK_FOLLOWS {
                        return Err(Errno::ELOOP);
                    }
                    let target = inode.read_link(&child_path.abs_path())?;
                    let next_path = join_symlink_target(
                        &target,
                        &nd.path_segments[nd.depth + 1..],
                        nd.requires_directory,
                    );
                    if next_path.len() > PATH_MAX {
                        return Err(Errno::ENAMETOOLONG);
                    }
                    validate_path_components(&next_path)?;
                    let next_base = if target.starts_with('/') {
                        task_root_path()
                    } else {
                        symlink_base
                    };
                    *nd = Nameidata::new_from_path(&next_path, next_base);
                    symlink_follows += 1;
                    continue 'restart;
                }
                nd.dentry = child_dentry;
            }
            nd.depth += 1;
        }

        return Ok(());
    }
}
