// os/src/fs.rs

// 实现参照
// 
// fs/
// ├── mod.rs              fs 总入口，初始化根文件系统，open_file/open，创建 /proc /dev /etc
// ├── ext4/
// │   ├── mod.rs          ext4 模块入口，创建全局 SUPER_BLOCK
// │   ├── super_block.rs  Ext4SuperBlock，负责挂载 lwext4，连接 Disk
// │   └── inode.rs        Ext4Inode，实现 fs::Inode
// ├── vfs/
// │   ├── mod.rs          定义 SuperBlock / Inode / File 三个核心 trait
// │   └── inode.rs        OSInode，实现 File，维护文件偏移
// ├── fsidx.rs            简单 inode 路径缓存，path -> Arc<dyn Inode>
// ├── path.rs             路径处理，绝对路径拼接、父子路径拆分
// ├── ffi.rs              OpenFlags / MountFlags / UmountFlags
// ├── stat.rs             Kstat，给 fstat/stat syscall 用
// ├── dirent.rs           目录项结构
// ├── mount.rs            简单挂载表
// ├── devfs.rs            /dev/zero、/dev/null、/dev/rtc 等伪设备
// ├── pipe.rs             管道，实现 File
// ├── stdio.rs            Stdin / Stdout，实现 File
// ├── preload.S           预加载 initproc
// └── inode.rs            旧 FAT32 路线代码，目前没有被 mod.rs 引入

pub mod ext4;
pub mod vfs;
mod kstat;
mod mount;
mod page_cache;
mod path;
mod fdtable;
mod stdio;
mod pipe;

pub use kstat::*;
pub use path::*;
use stdio::*;

// pub fn open()
