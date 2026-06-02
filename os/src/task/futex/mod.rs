// os/src/task/futex/mod.rs

mod queue;
mod wait;

pub use wait::*;

use crate::syscall::{Errno, SysResult};

/// 私有 futex 标志位：当设置此位时，futex 只对同一进程内的线程可见。
pub const FUTEX_PRIVATE_FLAG: usize = 128;
/// 提取 futex 命令（屏蔽 private/clock 标志）。
pub const FUTEX_CMD_MASK: usize = !(FUTEX_PRIVATE_FLAG | 256);

// 操作码
pub const FUTEX_WAIT: usize = 0;
pub const FUTEX_WAKE: usize = 1;
pub const FUTEX_WAIT_BITSET: usize = 9;
pub const FUTEX_WAKE_BITSET: usize = 10;
pub const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;

/// 执行 futex 系统调用的核心逻辑。
///
/// syscall 层只负责传入原始参数；具体 op 解析和 wait/wake 分发都放在 futex 模块内。
pub fn do_futex(
    uaddr: usize,
    futex_op: usize,
    val: usize,
    _timeout: usize,
    _uaddr2: usize,
    val3: usize,
) -> SysResult<usize> {
    let cmd = futex_op & FUTEX_CMD_MASK;
    let flags = futex_op & !FUTEX_CMD_MASK;

    if flags != 0 && flags != FUTEX_PRIVATE_FLAG {
        return Err(Errno::ENOSYS);
    }

    match cmd {
        FUTEX_WAIT => futex_wait(uaddr, val as u32),
        FUTEX_WAKE => futex_wake(uaddr, val as u32),
        FUTEX_WAIT_BITSET => futex_wait_bitset(uaddr, val as u32, val3 as u32),
        FUTEX_WAKE_BITSET => futex_wake_bitset(uaddr, val as u32, val3 as u32),
        _ => Err(Errno::ENOSYS),
    }
}
