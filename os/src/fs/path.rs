use alloc::sync::Arc;
use crate::syscall::{SysResult, Errno};
use super::vfs::InodeOp;

pub fn lookup_path(root: Arc<dyn InodeOp>, path: &str) -> SysResult<Arc<dyn InodeOp>> {
    if path == "/" {
        return Ok(root);
    }

    let mut cur = root;
    for comp in path.split('/').filter(|s| !s.is_empty()) {
        match comp {
            "." => continue,
            ".." => return Err(Errno::EINVAL), // 第一版可先不支持
            name => {
                cur = cur.lookup(name)?;
            }
        }
    }
    Ok(cur)
}
