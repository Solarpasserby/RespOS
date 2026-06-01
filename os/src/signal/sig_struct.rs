use super::sig_info::SigInfo;
use alloc::collections::btree_map::BTreeMap;
use bitflags::bitflags;
pub const MAX_SIGNUM: usize = 64;

pub struct SigPending {
    pub pending: SigSet, // 接收信号位图
    pub mask: SigSet,    // 信号掩码
    pub info: BTreeMap<i32, SigInfo>,
}

impl SigPending {
    pub fn new() -> Self {
        Self {
            pending: SigSet::empty(),
            mask: SigSet::empty(),
            info: BTreeMap::new(),
        }
    }

    pub fn with_mask(mask: SigSet) -> Self {
        Self {
            pending: SigSet::empty(),
            mask,
            info: BTreeMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.pending = SigSet::empty();
        self.mask = SigSet::empty();
        self.info.clear();
    }

    pub fn clear_pending(&mut self) {
        self.pending = SigSet::empty();
        self.info.clear();
    }

    // 添加一个新信号
    // 用作任务收到信号
    pub fn add_signal(&mut self, siginfo: SigInfo) {
        let sig = Sig::from(siginfo.signo); // 把i32 → 转换成 Sig 类型的信号
        self.pending.add_signal(sig);
        self.info.insert(siginfo.signo, siginfo);
    }

    // 获得信号信息
    pub fn get_info(&self, sig: Sig) -> Option<&SigInfo> {
        self.info.get(&sig.raw())
    }

    // 从当前待处理集合中选出最小的一个信号，但并不修改
    pub fn find_signal(&self) -> Option<Sig> {
        let mut temp_pending = self.pending.bits();
        loop {
            let pos: u32 = temp_pending.trailing_zeros();
            let sig = Sig::from((pos + 1) as i32);
            // 若全为0，则返回64，代表没有未决信号
            if pos == MAX_SIGNUM as u32 {
                return None;
            } else {
                temp_pending &= !(1 << pos);
                // 没有被屏蔽且无法屏蔽
                if !self.mask.contain_signal(sig)
                    || pos == Sig::SIGKILL.index() as u32
                    || pos == Sig::SIGSTOP.index() as u32
                {
                    break Some(Sig::from((pos + 1) as i32)); // 找到信号了，停止循环返回出去
                }
            }
        }
    }

    // 取出未处理集合中选出最小的一个信号，修改内容
    pub fn fetch_signal(&mut self) -> Option<(Sig, SigInfo)> {
        if let Some(sig) = self.find_signal() {
            debug!("[fetch_signal] fetch signal {}", sig.raw());
            self.pending.remove_signal(sig);
            Some((sig, self.info.remove(&sig.raw()).unwrap()))
        } else {
            None
        }
    }

    // 在信号掩码中添加新位
    pub fn add_mask(&mut self, sig: Sig) {
        self.mask.add_signal(sig);
    }

    // 在信号掩码中添加新位
    pub fn add_mask_sigset(&mut self, sigset: SigSet) {
        self.mask |= sigset;
    }

    // 换一个信号掩码
    pub fn change_mask(&mut self, mask: SigSet) -> SigSet {
        let old_mask = self.mask;
        self.mask = mask;
        old_mask
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Sig(i32);

impl Sig {
    pub const SIGHUP: Sig = Sig(1); // 终端挂断或控制进程死亡
    pub const SIGINT: Sig = Sig(2); // 键盘中断 (Ctrl+C)
    pub const SIGQUIT: Sig = Sig(3); // 键盘退出 (Ctrl+\)
    pub const SIGILL: Sig = Sig(4); // 非法指令
    pub const SIGTRAP: Sig = Sig(5); // 调试/断点陷阱
    pub const SIGABRT: Sig = Sig(6); // 程序异常中止
    pub const SIGBUS: Sig = Sig(7); // 总线错误(内存访问异常)
    pub const SIGFPE: Sig = Sig(8); // 浮点运算异常
    pub const SIGKILL: Sig = Sig(9); // 强制杀死进程
    pub const SIGUSR1: Sig = Sig(10); // 用户自定义信号1
    pub const SIGSEGV: Sig = Sig(11); // 非法内存访问(段错误)
    pub const SIGUSR2: Sig = Sig(12); // 用户自定义信号2
    pub const SIGPIPE: Sig = Sig(13); // 管道破裂(无读端却写入)
    pub const SIGALRM: Sig = Sig(14); // 闹钟超时
    pub const SIGTERM: Sig = Sig(15); // 终止信号(默认关闭)
    pub const SIGSTKFLT: Sig = Sig(16); // 协处理器栈错误(未使用)
    pub const SIGCHLD: Sig = Sig(17); // 子进程退出或暂停
    pub const SIGCONT: Sig = Sig(18); // 恢复暂停的进程
    pub const SIGSTOP: Sig = Sig(19); // 暂停进程
    pub const SIGTSTP: Sig = Sig(20); // 终端暂停信号(Ctrl+Z)
    pub const SIGTTIN: Sig = Sig(21); // 后台进程尝试读终端
    pub const SIGTTOU: Sig = Sig(22); // 后台进程尝试写终端
    pub const SIGURG: Sig = Sig(23); // Socket紧急数据到达
    pub const SIGXCPU: Sig = Sig(24); // 超过CPU时间限制
    pub const SIGXFSZ: Sig = Sig(25); // 超过文件大小限制
    pub const SIGVTALRM: Sig = Sig(26); // 虚拟时钟超时
    pub const SIGPROF: Sig = Sig(27); // 性能分析时钟超时
    pub const SIGWINCH: Sig = Sig(28); // 终端窗口大小改变
    pub const SIGIO: Sig = Sig(29); // I/O可进行信号
    pub const SIGPWR: Sig = Sig(30); // 电源故障
    pub const SIGSYS: Sig = Sig(31); // 非法系统调用
    pub const SIGLEGACYMAX: Sig = Sig(32); // 传统信号最大值
    pub const SIGMAX: Sig = Sig(64); // 系统支持的最大信号编号

    pub fn from(signum: i32) -> Sig {
        Sig(signum as i32)
    }

    pub fn is_valid(&self) -> bool {
        self.0 > 0 && self.0 <= MAX_SIGNUM as i32
    }

    pub fn raw(&self) -> i32 {
        self.0 as i32
    }

    pub fn index(&self) -> usize {
        (self.0 - 1) as usize
    }

    pub fn is_kill_or_stop(&self) -> bool {
        self.0 == Sig::SIGKILL.0 || self.0 == Sig::SIGSTOP.0
    }
}

// 数字太大会爆炸，这里内核用来传输信号，绝对安全
impl From<usize> for Sig {
    fn from(value: usize) -> Self {
        Sig(value as i32)
    }
}

bitflags! {
    pub struct SigSet: u64 {
        const SIGHUP    = 1 << 0 ;
        const SIGINT    = 1 << 1 ;
        const SIGQUIT   = 1 << 2 ;
        const SIGILL    = 1 << 3 ;
        const SIGTRAP   = 1 << 4 ;
        const SIGABRT   = 1 << 5 ;
        const SIGBUS    = 1 << 6 ;
        const SIGFPE    = 1 << 7 ;
        const SIGKILL   = 1 << 8 ;
        const SIGUSR1   = 1 << 9 ;
        const SIGSEGV   = 1 << 10;
        const SIGUSR2   = 1 << 11;
        const SIGPIPE   = 1 << 12;
        const SIGALRM   = 1 << 13;
        const SIGTERM   = 1 << 14;
        const SIGSTKFLT = 1 << 15;
        const SIGCHLD   = 1 << 16;
        const SIGCONT   = 1 << 17;
        const SIGSTOP   = 1 << 18;
        const SIGTSTP   = 1 << 19;
        const SIGTTIN   = 1 << 20;
        const SIGTTOU   = 1 << 21;
        const SIGURG    = 1 << 22;
        const SIGXCPU   = 1 << 23;
        const SIGXFSZ   = 1 << 24;
        const SIGVTALRM = 1 << 25;
        const SIGPROF   = 1 << 26;
        const SIGWINCH  = 1 << 27;
        const SIGIO     = 1 << 28;
        const SIGPWR    = 1 << 29;
        const SIGSYS    = 1 << 30;
        const SIGLEGACYMAX  = 1 << 31;


        const SIGRT1    = 1 << (33 - 1);   // 当前仅做「位图占位」，预留实时信号能力，内核后续可以直接启用。
        const SIGRT2    = 1 << (34 - 1);
        const SIGRT3    = 1 << (35 - 1);
        const SIGRT4    = 1 << (36 - 1);
        const SIGRT5    = 1 << (37 - 1);
        const SIGRT6    = 1 << (38 - 1);
        const SIGRT7    = 1 << (39 - 1);
        const SIGRT8    = 1 << (40 - 1);
        const SIGRT9    = 1 << (41 - 1);
        const SIGRT10    = 1 << (42 - 1);
        const SIGRT11    = 1 << (43 - 1);
        const SIGRT12   = 1 << (44 - 1);
        const SIGRT13   = 1 << (45 - 1);
        const SIGRT14   = 1 << (46 - 1);
        const SIGRT15   = 1 << (47 - 1);
        const SIGRT16   = 1 << (48 - 1);
        const SIGRT17   = 1 << (49 - 1);
        const SIGRT18   = 1 << (50 - 1);
        const SIGRT19   = 1 << (51 - 1);
        const SIGRT20   = 1 << (52 - 1);
        const SIGRT21   = 1 << (53 - 1);
        const SIGRT22   = 1 << (54 - 1);
        const SIGRT23   = 1 << (55 - 1);
        const SIGRT24   = 1 << (56 - 1);
        const SIGRT25   = 1 << (57 - 1);
        const SIGRT26   = 1 << (58 - 1);
        const SIGRT27   = 1 << (59 - 1);
        const SIGRT28   = 1 << (60 - 1);
        const SIGRT29   = 1 << (61 - 1);
        const SIGRT30   = 1 << (62 - 1);
        const SIGRT31   = 1 << (63 - 1);
        const SIGMAX   = 1 << 63;
    }
}

impl SigSet {
    pub fn add_signal(&mut self, sig: Sig) {
        self.insert(SigSet::from_bits(1 << sig.index()).unwrap())
    }

    pub fn contain_signal(&self, sig: Sig) -> bool {
        self.contains(SigSet::from_bits(1 << sig.index()).unwrap())
    }

    pub fn remove_signal(&mut self, sig: Sig) {
        self.remove(SigSet::from_bits(1 << sig.index()).unwrap())
    }
}

//一个信号 → 变成它对应的信号掩码（位图）
impl From<Sig> for SigSet {
    fn from(sig: Sig) -> Self {
        Self::from_bits(1 << sig.index()).unwrap()
    }
}
