use super::{MAX_SIGNUM, Sig, SigSet};
use bitflags::bitflags;

#[derive(Clone, Debug)]

// 用一个数组存下所有信号的处理规则，统一管理、查询、设置每个信号该做什么。
pub struct SigHandler {
    actions: [SigAction; MAX_SIGNUM],
}

impl SigHandler {
    // 把所有信号都设置成系统默认动作
    pub fn new() -> Self {
        Self {
            actions: core::array::from_fn(|signo| SigAction::new((signo + 1).into())),
        }
    }
    //传入一个信号，查询它现在的处理方式是什么
    pub fn get(&self, sig: Sig) -> SigAction {
        assert!(sig.is_valid());
        self.actions[sig.index()]
    }
    //修改某个信号的处理方式（不能改 SIGKILL/SIGSTOP）
    pub fn update(&mut self, sig: Sig, new: SigAction) {
        assert!(!sig.is_kill_or_stop());
        self.actions[sig.index()] = new;
    }
    //把所有信号的处理方式全部恢复成出厂默认，一键重置。
    pub fn reset(&mut self) {
        for (i, action) in self.actions.iter_mut().enumerate() {
            *action = SigAction::new(Sig::from((i + 1) as i32));
        }
    }

    // exec 后，用户自定义 handler 需要恢复默认；被忽略的信号保持忽略。
    pub fn reset_user_handlers_for_exec(&mut self) {
        for (i, action) in self.actions.iter_mut().enumerate() {
            if action.is_user() {
                *action = SigAction::new(Sig::from((i + 1) as i32));
            }
        }
    }
}

pub const SIG_DFL: usize = 0; // 默认行为
pub const SIG_IGN: usize = 1;

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct SigAction {
    pub sa_handler: usize,    // 信号处理函数指针
    pub flags: SigActionFlag, // 额外信息
    pub restorer: usize,      // 占位置、对齐内存
    pub mask: SigSet,         // 位掩码，临时阻塞信号
}

impl SigAction {
    pub fn new(sig: Sig) -> Self {
        //new(sig)只是出厂设置, 用户随后可以通过 sigaction 系统调用把它改掉。
        let atype = ActionType::default(sig);
        let sa_handler = match atype {
            ActionType::Ignore => SIG_IGN,
            ActionType::Term | ActionType::Stop | ActionType::Cont | ActionType::Core => SIG_DFL,
        };
        Self {
            sa_handler,
            flags: SigActionFlag::empty(),
            restorer: 0,
            mask: SigSet::empty(),
        }
    }

    pub fn is_user(&self) -> bool {
        let handler = self.sa_handler;
        (handler != 0) && (handler != 1)
    }
}

// 信号处理类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    Ignore, // 忽略信号
    Term,   // 终止进程
    Stop,   // 暂停进程
    Cont,   // 继续进程
    Core,   // 崩溃并保留错误信息
}

impl ActionType {
    // 信号默认处理方式
    pub fn default(sig: Sig) -> Self {
        match sig {
            Sig::SIGABRT
            | Sig::SIGBUS
            | Sig::SIGFPE
            | Sig::SIGILL
            | Sig::SIGQUIT
            | Sig::SIGSEGV
            | Sig::SIGTRAP
            | Sig::SIGXCPU
            | Sig::SIGXFSZ
            | Sig::SIGSYS => ActionType::Core,
            Sig::SIGSTOP | Sig::SIGTSTP | Sig::SIGTTIN | Sig::SIGTTOU => ActionType::Stop,
            Sig::SIGCHLD | Sig::SIGURG | Sig::SIGWINCH => ActionType::Ignore,
            Sig::SIGCONT => ActionType::Cont,
            _ => ActionType::Term,
        }
    }
}
bitflags! {
    pub struct SigActionFlag : u32 {
        const SA_NOCLDSTOP = 1;      // 子进程暂停时，不通知父进程
        const SA_NOCLDWAIT = 2;      // 子进程退出时，不变成僵尸进程，直接回收
        const SA_SIGINFO = 4;        // 使用高级信号处理函数（带详细信息），SA_SIGINFO 是开关，后续SigInfo 是开关打开后传的东西。一个标志位，一个数据体。
        const SA_ONSTACK = 0x08000000;// 使用备用栈处理信号
        const SA_RESTART = 0x10000000;// 被信号打断的系统调用自动重启
        const SA_NODEFER = 0x40000000;// 处理信号时，不屏蔽自己
        const SA_RESETHAND = 0x80000000;// 执行一次信号处理后，恢复默认行为
        const SA_RESTORER = 0x04000000;// 内核内部使用的恢复函数
    }
}
