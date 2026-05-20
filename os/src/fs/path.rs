// os/src/fs/pipe.rs

use super::vfs::Dentry;
use alloc::string::String;
use alloc::sync::Arc;

pub struct Path {
    pub dentry: Arc<Dentry>,
}

impl Path {
    pub fn new(/* mnt: Arc<VfsMount> ,*/ dentry: Arc<Dentry>) -> Arc<Self> {
        Arc::new(Path {
            // mnt,
            dentry,
        })
    }
    pub fn zero_init() -> Arc<Self> {
        Arc::new(Path {
            // mnt: Arc::new(VfsMount::zero_init()),
            dentry: Arc::new(Dentry::zero_init()),
        })
    }
    pub fn from_existed_user(path: &Arc<Path>) -> Arc<Self> {
        Arc::new(Path {
            // mnt: path.mnt.clone(),
            dentry: path.dentry.clone(),
        })
    }

    pub fn abs_path(&self) -> String {
        self.dentry.abs_path.clone()
    }
}
