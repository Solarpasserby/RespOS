// os/src/fs/kstat.rs

use super::vfs::InodeType;
use crate::config::BLOCK_SIZE;
use crate::timer::TimeSpec;

/// 内核对文件状态的描述
///
/// 当前实现相当简陋
#[derive(Clone, Debug)]
pub struct KStat {
    pub size: usize,
    pub ty: InodeType,
}

// #[repr(C)]
// #[derive(Default)]
// pub struct Kstat {
//     pub result_mask: u32,      // 指示哪些字段被填充
//     pub mode: u16,             // 文件权限和类型，如 S_IFREG, S_IFDIR
//     pub nlink: u32,            // 硬链接数
//     pub blksize: u32,          // I/O 块大小
//     pub attributes: u64,       // File attributes
//     pub attributes_mask: u64,  // Supported attributes mask
//     pub ino: u64,              // inode号(inode->i_ino)
//     pub dev: u64,              // 设备号(inode->i_sb->s_dev)
//     pub rdev: u64,             // Device ID (if special file)
//     pub uid: u32,              // Owner User ID of the file
//     pub gid: u32,              // Owner Group ID of the file
//     pub size: u64,             // File size (bytes)
//     pub atime: TimeSpec,       // Last access time
//     pub mtime: TimeSpec,       // Last modification time
//     pub ctime: TimeSpec,       // Last status change time
//     pub btime: TimeSpec,       // Creation time
//     pub blocks: u64,           // Number of 512B blocks allocated(inode->i_blocks)
//     pub mnt_id: u64,           // Mount ID
//     pub dio_mem_align: u32,    // DIO memory alignment
//     pub dio_offset_align: u32, // DIO offset alignment
//     pub change_cookie: u64,    // inode版本号
//     pub subvol: u64,           // 子卷ID
// }

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Stat {
    pub st_dev: u64,        // 设备 ID
    pub st_ino: u64,        // 索引节点号
    pub st_mode: u32,       // 文件类型和模式
    pub st_nlink: u32,      // 硬链接数
    pub st_uid: u32,        // 所有者的用户 ID
    pub st_gid: u32,        // 所有者的用户组 ID
    pub st_rdev: u64,       // 设备 ID （特殊文件）
    pub __pad: u64,         //
    pub st_size: u64,       // 文件总大小
    pub st_blksize: u32,    // 文件系统 I/O 的块大小
    pub __pad2: u32,        //
    pub st_blocks: u64,     // 分配的块数
    pub st_atime: TimeSpec, // 最近访问时间
    pub st_mtime: TimeSpec, // 最近修改时间
    pub st_ctime: TimeSpec, // 最近状态变化时间
    pub unused: u64,        //
}

/// 简单实现 [`KStat`] 到 [`Stat`] 的转换
impl From<KStat> for Stat {
    fn from(kstat: KStat) -> Self {
        let st_mode = ((kstat.ty as u32) << 12) | default_perm(kstat.ty);
        let st_size = kstat.size as u64;
        let st_blksize = BLOCK_SIZE as u32;
        let st_blocks = st_size.div_ceil(BLOCK_SIZE as u64);

        Self {
            st_dev: 0,
            st_ino: 0,
            st_mode,
            st_nlink: 1,
            st_uid: 0,
            st_gid: 0,
            st_rdev: 0,
            __pad: 0,
            st_size,
            st_blksize,
            __pad2: 0,
            st_blocks,
            st_atime: TimeSpec::default(),
            st_mtime: TimeSpec::default(),
            st_ctime: TimeSpec::default(),
            unused: 0,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Statfs64 {
    pub f_type: i64,
    pub f_bsize: i64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: [i32; 2],
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [usize; 4],
}

fn default_perm(ty: InodeType) -> u32 {
    match ty {
        InodeType::Directory => 0o755,
        InodeType::Regular => 0o644,
        _ => 0o666,
    }
}
