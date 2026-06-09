//!  select/pselect6 使用的 fd_set 位图。
//!
//!  fd_set 是一个大小为 `nfds` 位的位图，每位对应一个文件描述符编号。
//!  select 调用时用户传入感兴趣的 fd 集合（置位），内核遍历并将被置位
//!  且已就绪的 fd 保留在集合中写回用户空间。
//!
//!  流程：
//!  1. FdSet::from_user   — 将用户态 fd_set 拷贝到内核缓冲区
//!  2. init_fdset          — 遍历置位的 fd，解析出 Arc\<dyn FileOp\> 放入 FdSetIter
//!  3. 轮询就绪态，对就绪 fd 调用 fdset.set()
//!  4. fdset.write_back    — 将修改后的位图写回用户空间

use crate::fs::vfs::FileOp;
use crate::mm::{copy_from_user, copy_to_user};
use crate::syscall::{Errno, SysResult};
use crate::task::current_task;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

/// 每 word 可表示 64 位（32 位架构则为 32 位）
const FDSET_BITS_PER_WORD: usize = core::mem::size_of::<usize>() * 8;
/// Linux 默认 fd_set 最多容纳 1024 个 fd
const FDSET_MAX_FDS: usize = 1024;

/// 内核态 fd_set 副本，管理从用户空间拷贝来的位图缓冲区。
///
/// 位图以 `usize` 数组存储。`nfds` 是需要监控的 fd 范围上限，
/// 实际分配 `nfds / BITS_PER_WORD` 个 word。
pub struct FdSet {
    kernel_buf: Vec<usize>,
    /// 用户空间 fd_set 的地址，write_back 时写回此处
    user_addr: *mut usize,
    nfds: usize,
}

impl FdSet {
    /// 创建空的 FdSet（用于 writefds/exceptfds 为 NULL 的场景）
    pub fn new_empty() -> Self {
        FdSet {
            kernel_buf: Vec::new(),
            user_addr: core::ptr::null_mut(),
            nfds: 0,
        }
    }

    /// 从用户空间传入的 fd_set 位图拷贝到内核缓冲区。
    ///
    /// `addr` 是 `fd_set *`，`nfds` 是 `select(nfds, ...)` 的第一个参数。
    pub fn from_user(addr: usize, nfds: usize) -> Result<Self, Errno> {
        if nfds > FDSET_MAX_FDS {
            return Err(Errno::EINVAL);
        }
        let words = nfds.div_ceil(FDSET_BITS_PER_WORD);
        let mut kernel_buf = vec![0; words];
        if words > 0 {
            copy_from_user(kernel_buf.as_mut_ptr(), addr as *const usize, words)?;
        }
        Ok(FdSet {
            user_addr: addr as *mut usize,
            nfds,
            kernel_buf,
        })
    }

    /// 检查 fd 在 fd_set 中是否被置位
    pub fn check(&self, fd: usize) -> bool {
        if fd >= self.nfds {
            return false;
        }
        let word_index = fd / FDSET_BITS_PER_WORD;
        let bit_index = fd % FDSET_BITS_PER_WORD;
        self.kernel_buf[word_index] & (1usize << bit_index) != 0
    }

    /// 将 fd 在 fd_set 中置位——表示该 fd 就绪
    pub fn set(&mut self, fd: usize) {
        if fd >= self.nfds {
            return;
        }
        let word_index = fd / FDSET_BITS_PER_WORD;
        let bit_index = fd % FDSET_BITS_PER_WORD;
        self.kernel_buf[word_index] |= 1usize << bit_index;
    }

    /// 是否关联了有效的用户空间地址（NULL 表示用户不关心该集合）
    pub fn valid(&self) -> bool {
        !self.user_addr.is_null()
    }

    /// 清零位图——在每轮轮询前将上次的就绪结果清掉
    pub fn clear(&mut self) {
        for word in self.kernel_buf.iter_mut() {
            *word = 0;
        }
    }

    /// 将内核修改后的 fd_set 写回用户空间
    pub fn write_back(&self) -> SysResult<usize> {
        copy_to_user(
            self.user_addr,
            self.kernel_buf.as_ptr(),
            self.kernel_buf.len(),
        )
    }
}

/// 就绪轮询迭代器：持有 fd_set 位图 + 已解析的文件对象列表。
///
/// 初始构造时遍历 fd_set 中所有置位的 fd，将其对应的 `FileOp` 取出。
/// 后续轮询只需遍历这个列表即可，无需重复查 fd 表。
pub struct FdSetIter {
    pub fdset: FdSet,
    pub files: Vec<Arc<dyn FileOp>>,
    pub fds: Vec<usize>,
}

/// 从用户态 fd_set 构造 FdSetIter。
///
/// `addr == 0` 表示用户传入 NULL（不关心该集合），返回空的 FdSetIter。
/// 否则拷贝位图 → 遍历置位 fd → 解析 FileOp → 清零位图准备就绪标记。
pub fn init_fdset(addr: usize, nfds: usize) -> Result<FdSetIter, Errno> {
    if nfds > FDSET_MAX_FDS {
        return Err(Errno::EINVAL);
    }
    if addr == 0 {
        return Ok(FdSetIter {
            fdset: FdSet::new_empty(),
            files: Vec::new(),
            fds: Vec::new(),
        });
    }
    let mut fdset = FdSet::from_user(addr, nfds)?;
    let task = current_task().expect("[kernel] current task is None.");
    let mut files: Vec<Arc<dyn FileOp>> = Vec::new();
    let mut fds: Vec<usize> = Vec::new();
    for fd in 0..nfds {
        if fdset.check(fd) {
            let file = task.get_fd_entry(fd)?.file;
            files.push(file);
            fds.push(fd);
        }
    }
    // 清空位图：原置位信息已保存在 fds 列表中，位图现在用于标记就绪 fd
    fdset.clear();
    Ok(FdSetIter { fdset, files, fds })
}
