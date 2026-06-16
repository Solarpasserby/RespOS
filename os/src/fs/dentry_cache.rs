// os/src/fs/dentry_cache.rs

use super::vfs::Dentry;
use crate::config::DENTRY_CACHE_CAPACITY;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use hashbrown::HashMap;
use lazy_static::lazy_static;
use spin::Mutex;

lazy_static! {
    /// 全局 dentry 缓存，key 为绝对路径
    static ref DENTRY_CACHE: Mutex<HashMap<String, Arc<Dentry>>> = Mutex::new(HashMap::new());
    /// 固有 dentry 锚点，持强引用确保其 `strong_count >= 2`，永不被缓存淘汰
    static ref VFS_DENTRY_ANCHORS: Mutex<Vec<Arc<Dentry>>> = Mutex::new(Vec::new());
}

/// 查全局 dentry 缓存，命中返回 dentry，未命中返回 None
pub fn lookup_dentry_cache(abs_path: &str) -> Option<Arc<Dentry>> {
    DENTRY_CACHE.lock().get(abs_path).cloned()
}

/// 将 dentry 插入全局缓存，若已满则踢掉一个只有缓存引用的条目腾位
pub fn insert_dentry_cache(dentry: Arc<Dentry>) {
    let mut cache = DENTRY_CACHE.lock();
    if cache.len() >= DENTRY_CACHE_CAPACITY {
        let victim_key = cache
            .iter()
            .find(|(_, d)| Arc::strong_count(d) == 1)
            .map(|(k, _)| k.clone());
        if let Some(key) = victim_key {
            cache.remove(&key);
        }
    }
    cache.insert(dentry.abs_path.clone(), dentry);
}

/// 将 dentry 加入固有锚点列表，使其永不被淘汰
pub fn pin_vfs_dentry(dentry: Arc<Dentry>) {
    VFS_DENTRY_ANCHORS.lock().push(dentry);
}

/// 从全局缓存移除指定路径的 dentry
pub fn remove_dentry_cache(abs_path: &str) {
    DENTRY_CACHE.lock().remove(abs_path);
}

/// 从全局缓存移除指定路径及其子路径的 dentry
pub fn remove_dentry_cache_tree(abs_path: &str) {
    let mut cache = DENTRY_CACHE.lock();
    cache.remove(abs_path);

    let mut prefix = String::from(abs_path);
    if !prefix.ends_with('/') {
        prefix.push('/');
    }

    let keys: Vec<String> = cache
        .keys()
        .filter(|path| path.starts_with(&prefix))
        .cloned()
        .collect();
    for key in keys {
        cache.remove(&key);
    }
}

/// 回收所有只被缓存引用的 dentry（物理页不够时调用）
pub fn clean_dentry_cache() {
    let mut cache = DENTRY_CACHE.lock();
    let keys: Vec<String> = cache
        .iter()
        .filter(|(_, d)| Arc::strong_count(d) == 1)
        .map(|(k, _)| k.clone())
        .collect();
    for key in keys {
        cache.remove(&key);
    }
}
