#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SigInfo {
    pub signo: i32,      // 信号值
    pub code: i32,       // 信号产生原因
    pub fields: SiField, // 额外信息
}

impl SigInfo {
    pub fn new(signo: i32, code: i32, field: SiField) -> Self {
        Self {
            signo,
            code,
            fields: field,
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub enum SiField {
    None,
    Kill { tid: usize }, //这里填的是发送者身份，不一定是线程号
}

#[allow(unused)]
impl SigInfo {
    /// 由 kill、sigsend、raise 发送
    pub const USER: i32 = 0;
    /// 由内核在特定场景下发送
    pub const KERNEL: i32 = 0x80;
    /// 由 sigqueue 发送
    pub const QUEUE: i32 = -1;
    /// 由定时器到期发送
    pub const TIMER: i32 = -2;
    /// 由实时消息队列状态变更发送
    pub const MESGQ: i32 = -3;
    /// 由 AIO 完成事件发送
    pub const ASYNCIO: i32 = -4;
    /// 由排队的 SIGIO 发送
    pub const SIGIO: i32 = -5;
    /// 由 tkill 系统调用发送
    pub const TKILL: i32 = -6;
    /// 由 execve() 终止辅助线程时发送
    pub const DETHREAD: i32 = -7;
    /// 由 glibc 异步名称解析完成发送
    pub const ASYNCNL: i32 = -60;

    // SIGCHLD 的 si_codes 定义
    /// 子进程已退出
    pub const CLD_EXITED: i32 = 1;
    /// 子进程被杀死
    pub const CLD_KILLED: i32 = 2;
    /// 子进程异常终止（产生 core dump）
    pub const CLD_DUMPED: i32 = 3;
    /// 被跟踪的子进程已陷入
    pub const CLD_TRAPPED: i32 = 4;
    /// 子进程已暂停
    pub const CLD_STOPPED: i32 = 5;
    /// 已暂停的子进程已继续运行
    pub const CLD_CONTINUED: i32 = 6;
    pub const NSIGCHLD: i32 = 6;
}

#[derive(Default, Copy, Clone)]
#[repr(C)]
//写到用户栈上给用户程序读
pub struct LinuxSigInfo {
    pub si_signo: i32,
    pub si_errno: i32,
    pub si_code: i32,
    pub _pad: [i32; 29], // 占位填充，保持结构体大小与 Linux 一致
    _align: [u64; 0],    // 零大小对齐标记
}
