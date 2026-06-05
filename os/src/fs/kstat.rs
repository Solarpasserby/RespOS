// os/src/fs/kstat.rs

use super::vfs::InodeType;
use crate::config::BLOCK_SIZE;
use crate::timer::TimeSpec;

/// 内核对文件状态的描述
#[derive(Clone, Debug)]
pub struct KStat {
    pub dev: u64,
    pub size: usize,
    pub ty: InodeType,
    pub ino: u64,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
    pub mode: u32,
    pub blksize: u32,
    pub blocks: u64,
    pub atime: TimeSpec,
    pub mtime: TimeSpec,
    pub ctime: TimeSpec,
}

impl KStat {
    const STAT_BLOCK_SIZE: u64 = 512;

    pub fn minimal(size: usize, ty: InodeType) -> Self {
        Self {
            dev: 0,
            size,
            ty,
            ino: 0,
            nlink: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
            mode: 0,
            blksize: BLOCK_SIZE as u32,
            blocks: Self::blocks_for_size(size as u64),
            atime: TimeSpec::default(),
            mtime: TimeSpec::default(),
            ctime: TimeSpec::default(),
        }
    }

    pub fn with_ino(mut self, ino: u64) -> Self {
        self.ino = ino;
        self
    }

    pub fn with_dev(mut self, dev: u64) -> Self {
        self.dev = dev;
        self
    }

    pub fn with_mode(mut self, mode: u32) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_nlink(mut self, nlink: u32) -> Self {
        self.nlink = nlink;
        self
    }

    pub fn with_rdev(mut self, rdev: u64) -> Self {
        self.rdev = rdev;
        self
    }

    pub fn blocks_for_size(size: u64) -> u64 {
        size.div_ceil(Self::STAT_BLOCK_SIZE)
    }
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
        let st_mode = file_type_mode(kstat.ty) | file_perm_mode(kstat.ty, kstat.mode);
        let st_size = kstat.size as u64;
        let st_blksize = kstat.blksize;
        let st_blocks = kstat.blocks;

        Self {
            st_dev: kstat.dev,
            st_ino: kstat.ino,
            st_mode,
            st_nlink: kstat.nlink.max(1),
            st_uid: kstat.uid,
            st_gid: kstat.gid,
            st_rdev: kstat.rdev,
            __pad: 0,
            st_size,
            st_blksize,
            __pad2: 0,
            st_blocks,
            st_atime: kstat.atime,
            st_mtime: kstat.mtime,
            st_ctime: kstat.ctime,
            unused: 0,
        }
    }
}

fn file_type_mode(ty: InodeType) -> u32 {
    (ty as u32) << 12
}

fn file_perm_mode(ty: InodeType, mode: u32) -> u32 {
    const S_IFMT: u32 = 0o170000;
    if mode & S_IFMT != 0 {
        mode & !S_IFMT
    } else if mode != 0 {
        mode
    } else {
        default_perm(ty)
    }
}

fn default_perm(ty: InodeType) -> u32 {
    match ty {
        InodeType::Directory => 0o755,
        InodeType::Regular => 0o644,
        InodeType::SymLink => 0o777,
        _ => 0o666,
    }
}
