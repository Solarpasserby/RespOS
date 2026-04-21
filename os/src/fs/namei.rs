// os/src/fs/namei.rs

use alloc::vec::Vec;
use alloc::sync::Arc;
use crate::task::current_task;
use crate::syscall::{SysResult, Errno};
use super::vfs::{Dentry, ROOT_DENTRY, File};
use super::Path;

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

// pub fn lookup_dentry(nd: &mut Nameidata) -> Arc<Dentry> {
//     let mut absolute_current_dir = nd.dentry.absolute_path.clone();
//     absolute_current_dir = absolute_current_dir + "/" + nd.path_segments[nd.depth];
//     let mut dentry = lookup_dcache_with_absolute_path(&absolute_current_dir);
//     if dentry.is_none() {
//         let current_dir_inode = nd.dentry.get_inode();
//         dentry = Some(current_dir_inode.lookup(&nd.path_segments[nd.depth], nd.dentry.clone()));
//         // 注意这里插入的dentry可能是负目录项
//     }
//     let dentry = dentry.unwrap();
//     insert_dentry(dentry.clone());
//     log::info!(
//         "[lookup_dentry] dentry: {:?}, is_negative: {}",
//         dentry.absolute_path,
//         dentry.is_negative()
//     );
//     dentry
// }

pub fn path_open(path: &str, flags: usize, mode: usize) -> SysResult<Arc<File>> {
    let mut nd = Nameidata::new(path);

    Err(Errno::ENOSYS)
}

pub fn link_path_walk(nd: &mut Nameidata) -> SysResult {
    println!("[kernel] func:link_path_walk path: {:?}", nd.path_segments);
    let len = nd.path_segments.len() - 1;
    while nd.depth < len {
        let name = nd.path_segments[nd.depth];
        if name == "." {
            //do nothing
        } else if name == ".." {
            let parent_dentry = nd.dentry.get_parent_or_self();
            nd.dentry = parent_dentry;
        } else {

        }
        // 统一推进解析深度
        nd.depth += 1
    }
    Ok(())
}