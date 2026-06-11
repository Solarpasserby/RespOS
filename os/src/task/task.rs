// os/src/task/task.rs
use super::INITPROC;
use super::aux::{AT_EXECFN, AT_NULL, AT_PLATFORM, AT_RANDOM, AuxHeader};
use super::context::TaskContext;
use super::kstack::KernelStack;
use super::manager::TASK_MANAGER;
use super::scheduler::remove_task;
use super::tid::{TidHandle, tid_alloc};
use crate::fs::mount::init_root_fs;
use crate::fs::{FdEntry, FdTable, Path};
use crate::mm::{MemorySet, copy_from_user, copy_to_user};
use crate::mutex::SpinLock;
use crate::signal::sig_handler::SigHandler;
use crate::signal::sig_info::SigInfo;
use crate::signal::sig_stack::{SS_DISABLE, SignalStack};
use crate::signal::sig_struct::SigPending;
use crate::signal::{SiField, Sig};
use crate::syscall::{Errno, SysResult};
use crate::trap::TrapContext;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use bitflags::bitflags;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use spin::RwLock;

/// 线程 tid 地址信息，用于 pthread 线程退出同步。
pub struct TidAddress {
    /// 当 CLONE_CHILD_SETTID 被设置时，新线程将其 TID 写入此地址。
    pub set_child_tid: Option<usize>,
    /// 线程退出时清零并做 futex wake 的用户空间地址。
    pub clear_child_tid: Option<usize>,
    /// set_robust_list 注册的 robust_list_head 地址。
    pub robust_list_head: Option<usize>,
    /// robust_list_head 结构长度。
    pub robust_list_len: usize,
}

impl TidAddress {
    pub fn new() -> Self {
        Self {
            set_child_tid: None,
            clear_child_tid: None,
            robust_list_head: None,
            robust_list_len: 0,
        }
    }
}

/// 任务控制块——此处的任务是对一定资源和某个程序的抽象表述
#[repr(C)]
pub struct TaskControlBlock {
    // 固定数据
    kernel_stack: KernelStack, // 对于当前实现，确保 `TaskControlBlock` 的第一个字段为内核栈

    // 基本数据
    tid: RwLock<TidHandle>,
    tgid: AtomicUsize,
    // pgid: AtomicUsize,
    thread_group: Arc<SpinLock<ThreadGroup>>,
    task_status: SpinLock<TaskStatus>,
    parent: Arc<SpinLock<Option<Weak<TaskControlBlock>>>>,
    children: Arc<SpinLock<BTreeMap<usize, Arc<TaskControlBlock>>>>,
    exit_code: AtomicI32,
    exit_signal: AtomicI32,
    // task_context: TaskContext, // 注意任务上下文的处理

    // 内存管理
    memory_set: Arc<RwLock<MemorySet>>,

    // 文件系统
    fd_table: SpinLock<Arc<FdTable>>,
    cwd: Arc<SpinLock<Arc<Path>>>,
    exe_path: Arc<SpinLock<String>>,

    //信号
    sig_pending: SpinLock<SigPending>, // 本线程的信号队列 + 掩码（独享）
    sig_stack: SpinLock<SignalStack>,  // 本线程的备用信号栈（独享）
    sig_handler: Arc<SpinLock<SigHandler>>, // 线程组共享的 handler 注册表（共享）
    sig_context_addr: AtomicUsize,     // 用户栈上 SigContext 的地址

    // 线程同步
    tid_address: SpinLock<TidAddress>,
    // ===== 新增：可中断状态标记 =====
    // 标记当前线程是否处于"可被信号中断"的阻塞中（futex_wait / sigtimedwait / wait4）
    interruptible: AtomicBool,
    // 信号中断标记：当线程在 interruptible 状态下被信号唤醒时置为 true
    interrupted: AtomicBool,
}

impl core::fmt::Debug for TaskControlBlock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Task")
            .field("tid", &self.tid())
            .field("tgid", &self.tgid())
            .finish()
    }
}

impl TaskControlBlock {
    /// 全零初始化
    pub fn zero_init() -> Self {
        Self {
            // 固定数据
            kernel_stack: KernelStack::zero_init(),

            // 基本数据
            tid: RwLock::new(TidHandle(0)),
            tgid: AtomicUsize::new(0),
            // pgid: AtomicUsize,
            thread_group: Arc::new(SpinLock::new(ThreadGroup::new())),
            task_status: SpinLock::new(TaskStatus::Ready),
            parent: Arc::new(SpinLock::new(None)),
            children: Arc::new(SpinLock::new(BTreeMap::new())),
            exit_code: AtomicI32::new(0),
            exit_signal: AtomicI32::new(0),
            // task_context: TaskContext, // 注意任务上下文的处理

            // 内存管理
            memory_set: Arc::new(RwLock::new(MemorySet::new())),

            // 文件系统
            fd_table: SpinLock::new(FdTable::new()),
            cwd: Arc::new(SpinLock::new(Path::zero_init())),
            exe_path: Arc::new(SpinLock::new(String::new())),

            //信号
            sig_pending: SpinLock::new(SigPending::new()),
            sig_stack: SpinLock::new(SignalStack::default()),
            sig_handler: Arc::new(SpinLock::new(SigHandler::new())),
            sig_context_addr: AtomicUsize::new(0),

            // 线程同步
            tid_address: SpinLock::new(TidAddress::new()),

            // 可中断状态
            interruptible: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
        }
    }

    /// 新建任务
    ///
    /// 事实上只有初始任务会借由这个方法产生
    ///
    pub fn init(elf_data: &[u8]) -> Arc<Self> {
        let tid: TidHandle = tid_alloc();
        let tgid = tid.0;
        // 创建地址空间会拷贝内核页表，先创建内核栈生成页表映射，以保证任务切换后能正确访问内核栈
        let mut kernel_stack = KernelStack::new(&tid);
        let (memory_set, token, user_sp, entry_point, _aux_vec) =
            MemorySet::from_elf_data(elf_data);

        let mut kernel_stack_top = kernel_stack.get_top(); // 由于栈是新建的，栈顶就是栈顶边界
        // 在栈上存储异常上下文，该数据不会从栈中弹出，固定位于栈最高位置
        kernel_stack_top -= core::mem::size_of::<TrapContext>();
        let trap_cx_ptr = kernel_stack_top as *mut TrapContext;
        // 在栈上设置任务上下文，使任务可被正常切换
        kernel_stack_top -= core::mem::size_of::<TaskContext>();
        let task_cx_ptr = kernel_stack_top as *mut TaskContext;
        // 重新设置栈顶指针
        kernel_stack.set_top(kernel_stack_top);

        // 创建进程控制块
        let task_ctrl_block = Arc::new(Self {
            // 固定数据
            kernel_stack,

            // 基本数据
            tid: RwLock::new(tid),
            tgid: AtomicUsize::new(tgid),
            // pgid: 0,
            thread_group: Arc::new(SpinLock::new(ThreadGroup::new())),
            task_status: SpinLock::new(TaskStatus::Ready),
            parent: Arc::new(SpinLock::new(None)),
            children: Arc::new(SpinLock::new(BTreeMap::new())),
            exit_code: AtomicI32::new(0),
            exit_signal: AtomicI32::new(0),

            // 内存管理
            memory_set: Arc::new(RwLock::new(memory_set)),

            // 文件系统
            fd_table: SpinLock::new(FdTable::new()),
            cwd: Arc::new(SpinLock::new(init_root_fs())),
            exe_path: Arc::new(SpinLock::new(String::new())),

            //信号
            sig_pending: SpinLock::new(SigPending::new()),
            sig_stack: SpinLock::new(SignalStack::default()),
            sig_handler: Arc::new(SpinLock::new(SigHandler::new())),
            sig_context_addr: AtomicUsize::new(0),

            // 线程同步
            tid_address: SpinLock::new(TidAddress::new()),

            // 可中断状态
            interruptible: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
        });

        // 在线程组中添加该线程
        task_ctrl_block
            .thread_group
            .lock()
            .add(task_ctrl_block.clone());

        // 初始化内核栈上的异常上下文
        let trap_context = TrapContext::init_app_context(entry_point, user_sp, 0, 0, 0, 0, false);
        // 初始化任务上下文
        let task_context = TaskContext::app_init_task_context(token);

        // 修改内核栈中上下文数据
        unsafe {
            trap_cx_ptr.write(trap_context);
            task_cx_ptr.write(task_context);
        }

        // 在任务管理器中添加线程号到线程的映射
        TASK_MANAGER.add(&task_ctrl_block);

        task_ctrl_block
    }

    /// 克隆父线程，创建子线程
    pub fn clone_(self: &Arc<Self>, flags: CloneFlags) -> Arc<Self> {
        let tid = tid_alloc();

        // 克隆内核栈
        let mut kernel_stack = KernelStack::new(&tid);
        let mut kernel_stack_top = kernel_stack.get_top(); // 由于栈是新建的，栈顶就是栈顶边界
        kernel_stack_top -= core::mem::size_of::<TrapContext>();
        self.clone_trap_cx(kernel_stack_top);
        kernel_stack_top -= core::mem::size_of::<TaskContext>();
        // 注意这里只修改了栈指针但没有修改栈上的任务上下文，这需要在创建完任务控制块后再调用相关函数
        kernel_stack.set_top(kernel_stack_top);

        let is_thread = flags.contains(CloneFlags::CLONE_THREAD);

        let process_leader = self
            .op_thread_group(|tg| tg.iter().find(|task| task.tid() == self.tgid()))
            .unwrap_or_else(|| self.clone());

        // 创建线程或是进程
        let (tgid, thread_group, parent, children, cwd, exe_path) = if is_thread {
            // 创建线程，属于同一线程组
            (
                self.tgid(),
                self.thread_group.clone(),
                self.parent.clone(),
                self.children.clone(),
                self.cwd.clone(),
                self.exe_path.clone(),
            )
        } else {
            // 创建进程
            (
                tid.0,
                Arc::new(SpinLock::new(ThreadGroup::new())),
                Arc::new(SpinLock::new(Some(Arc::downgrade(&process_leader)))),
                Arc::new(SpinLock::new(BTreeMap::new())),
                Arc::new(SpinLock::new(Path::from_existed_user(&self.cwd()))),
                Arc::new(SpinLock::new(self.exe_path())),
            )
        };

        // 是否与父线程共享地址空间
        let memory_set = if flags.contains(CloneFlags::CLONE_VM) {
            // TODO: exec 替换地址空间时可能会出现问题
            self.memory_set.clone()
        } else {
            Arc::new(RwLock::new(MemorySet::from_existed_user(
                &mut self.memory_set.write(),
            )))
        };

        // 是否与父线程共享文件数据
        let fd_table = if is_thread || flags.contains(CloneFlags::CLONE_FILES) {
            self.fd_table.lock().clone()
        } else {
            FdTable::from_existed_user(&self.fd_table.lock())
        };
        let current_sig_mask = self.op_sig_pending(|pending| pending.mask);
        let (sig_handler, sig_pending, sig_stack) = if is_thread {
            (
                self.sig_handler.clone(), // 共享同一张 handler 表
                SpinLock::new(SigPending::with_mask(current_sig_mask)), // 自己的队列，继承当前 mask
                SpinLock::new(SignalStack::default()), // 自己的栈
            )
        } else {
            (
                Arc::new(SpinLock::new(
                    self.op_sig_handler(|handler| handler.clone()),
                )),
                SpinLock::new(SigPending::with_mask(current_sig_mask)),
                SpinLock::new(*self.sig_stack.lock()),
            )
        };
        let task_ctrl_block = Arc::new(TaskControlBlock {
            // 固定数据
            kernel_stack,

            // 基本数据
            tid: RwLock::new(tid),
            tgid: AtomicUsize::new(tgid),
            // pgid: 0,
            thread_group,
            task_status: SpinLock::new(TaskStatus::Ready),
            parent,
            children,
            exit_code: AtomicI32::new(0),
            exit_signal: AtomicI32::new(0),

            // 内存管理
            memory_set,

            // 文件系统
            fd_table: SpinLock::new(fd_table),
            cwd,
            exe_path,

            // 信号
            sig_pending,
            sig_stack,
            sig_handler,
            sig_context_addr: AtomicUsize::new(0),

            // 线程同步
            tid_address: SpinLock::new(TidAddress::new()),

            // 可中断状态
            interruptible: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
        });

        // 修改任务异常上下文
        task_ctrl_block.write_task_cx(kernel_stack_top);

        // 只有新进程进入 children；同线程组内的新线程不由 wait4 回收。
        if !is_thread {
            self.add_child(task_ctrl_block.clone());
        }
        // 在线程组中添加线程
        task_ctrl_block.op_thread_group_mut(|tg| tg.add(task_ctrl_block.clone()));

        // 在任务管理器中添加线程号到线程的映射
        TASK_MANAGER.add(&task_ctrl_block);

        task_ctrl_block
    }

    /// 载入可执行程序，主要修改地址空间、用户栈、异常上下文等数据
    ///
    /// 将命令行参数个数 `argc` 作为返回值，考虑到系统调用异常时会统一修改 `a0` 寄存器
    pub fn execve(
        self: &Arc<Self>,
        exe_path: String,
        elf_data: &[u8],
        args: Vec<String>,
        envs: Vec<String>,
        linux_abi: bool,
    ) -> SysResult<usize> {
        // 简化模型：只有进程 leader 可以 exec，避免非 leader exec 后父子关系和 tgid 语义混乱。
        if !self.is_process_leader() {
            return Err(Errno::EINVAL);
        }

        // 记录可执行文件路径，供 /proc/self/exe 使用
        self.set_exe_path(exe_path);

        let (memory_set, _token, mut user_sp, entry_point, aux_vec) =
            MemorySet::from_elf_data(elf_data);

        /* ===== 修改地址空间 ===== */
        let mut memory_set_guard = self.memory_set.write();
        let old_memory_set = core::mem::replace(&mut *memory_set_guard, memory_set);
        // 刷新页表，由于应用程序通过异常进入，在异常返回时不会刷新页表
        // 为了程序返回后看到的地址空间为自身而非父任务的地址空间，需要主动刷新页表
        memory_set_guard.activate();
        /* ===== 修改用户栈数据 ===== */
        let (argv_base, envp_base, auxv_base, stack_top) = init_user_stack(
            &mut memory_set_guard,
            args.as_slice(),
            envs.as_slice(),
            aux_vec,
            &mut user_sp,
        )?;
        drop(old_memory_set);
        drop(memory_set_guard);

        /* ===== 修改异常上下文 ===== */
        let argc = args.len();
        let trap_cx = self.get_trap_cx();
        *trap_cx = TrapContext::init_app_context(
            entry_point,
            stack_top,
            argc,
            argv_base,
            envp_base,
            auxv_base,
            linux_abi,
        );

        /* ===== 修改线程组 ===== */
        self.close_other_threads_for_exec();

        /* ===== 修改文件描述符表 ===== */
        // exec 保留 fd_table；后续可在这里处理 close-on-exec。

        /* ===== 修改信号处理 ===== */
        self.op_sig_handler_mut(|handler| handler.reset_user_handlers_for_exec());
        self.op_sig_pending_mut(|pending| pending.clear_pending());
        *self.sig_stack.lock() = SignalStack::default();

        Ok(argc)
    }
}

impl TaskControlBlock {
    /* ======= 获取内部数据 ====== */
    /// 线程号
    pub fn tid(&self) -> usize {
        self.tid.read().0
    }
    /// 线程组号
    pub fn tgid(&self) -> usize {
        self.tgid.load(Ordering::Relaxed)
    }
    pub fn status(&self) -> TaskStatus {
        self.task_status.lock().clone()
    }
    pub fn cwd(&self) -> Arc<Path> {
        self.cwd.lock().clone()
    }
    pub fn exe_path(&self) -> String {
        self.exe_path.lock().clone()
    }
    pub fn exit_code(&self) -> i32 {
        self.exit_code.load(Ordering::Relaxed)
    }
    pub fn wait_status(&self) -> i32 {
        let signal = self.exit_signal.load(Ordering::Relaxed);
        if signal != 0 {
            signal & 0x7f
        } else {
            (self.exit_code() & 0xff) << 8
        }
    }
    /// 获取用户任务页表的页表基址寄存器值
    pub fn get_user_token(&self) -> usize {
        self.memory_set.read().token()
    }
    // 任务状态判断
    pub fn is_ready(&self) -> bool {
        self.status() == TaskStatus::Ready
    }
    pub fn is_blocked(&self) -> bool {
        self.status() == TaskStatus::Blocked
    }
    // ===== 可中断状态管理 =====

    /// 进入可中断的阻塞前调用
    pub fn set_interruptible(&self, val: bool) {
        self.interruptible.store(val, Ordering::Relaxed);
    }

    /// 是否处于可中断状态
    fn is_interruptible(&self) -> bool {
        self.interruptible.load(Ordering::Relaxed)
    }

    /// 信号中断唤醒后检查
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }

    /// 清除中断标记（处理完 EINTR 后调用）
    pub fn clear_interrupted(&self) {
        self.interrupted.store(false, Ordering::Relaxed);
    }

    /// ★ 核心判断：当前线程是否应该被 pending 信号中断
    /// 条件：
    ///   1. 线程处于可中断状态 (interruptible == true)
    ///   2. 存在 pending 信号没有被掩码屏蔽
    ///   3. 该信号的处理程序不是 SIG_IGN（sa_handler != 1）
    pub fn check_signal_interrupt(&self) -> bool {
        use crate::signal::sig_handler::SIG_IGN;
        if !self.is_interruptible() {
            return false;
        }
        // find_signal 已经帮我们跳过了被 mask 屏蔽的信号
        if let Some(sig) = self.op_sig_pending(|pending| pending.find_signal()) {
            let action = self.op_sig_handler(|handler| handler.get(sig));
            // sa_handler == SIG_IGN(1) → 信号被显式忽略，不需要打断
            action.sa_handler != SIG_IGN
        } else {
            false
        }
    }
    pub fn is_exited(&self) -> bool {
        self.status() == TaskStatus::Exited
    }
    pub fn is_process_leader(&self) -> bool {
        self.tid() == self.tgid()
    }
    /* ======= 设置内部数据 ====== */
    pub fn set_tgid(&self, tgid: usize) {
        self.tgid.swap(tgid, Ordering::Relaxed);
    }
    pub fn set_cwd(&self, path: Arc<Path>) {
        *self.cwd.lock() = path;
    }
    pub fn set_exe_path(&self, path: String) {
        *self.exe_path.lock() = path;
    }
    pub fn set_exit_code(&self, exit_code: i32) {
        self.exit_signal.store(0, Ordering::Relaxed);
        self.exit_code.swap(exit_code, Ordering::Relaxed);
    }
    pub fn set_exit_signal(&self, signal: i32) {
        self.exit_code.store(0, Ordering::Relaxed);
        self.exit_signal.store(signal & 0x7f, Ordering::Relaxed);
    }
    pub fn set_parent(&self, parent: &Arc<TaskControlBlock>) {
        *self.parent.lock() = Some(Arc::downgrade(parent));
    }
    // 添加子任务
    pub fn add_child(&self, task: Arc<TaskControlBlock>) {
        // TODO: 返回结果是个 `Result`，之后可能要修改实现统一返回 `SysResult`
        let tid = task.tid();
        self.children.lock().insert(tid, task);
    }
    // 任务状态设置
    pub fn set_ready(&self) {
        *self.task_status.lock() = TaskStatus::Ready;
    }
    pub fn set_running(&self) {
        *self.task_status.lock() = TaskStatus::Running;
    }
    pub fn set_blocked(&self) {
        *self.task_status.lock() = TaskStatus::Blocked;
    }
    pub fn set_exited(&self) {
        *self.task_status.lock() = TaskStatus::Exited;
    }

    // tid_address 设置
    pub fn set_clear_child_tid(&self, addr: usize) {
        self.tid_address.lock().clear_child_tid = (addr != 0).then_some(addr);
    }

    pub fn set_set_child_tid(&self, addr: usize) {
        self.tid_address.lock().set_child_tid = Some(addr);
    }

    pub fn clear_child_tid_addr(&self) -> Option<usize> {
        self.tid_address.lock().clear_child_tid
    }
    pub fn set_robust_list(&self, head: usize, len: usize) {
        let mut tid_address = self.tid_address.lock();
        tid_address.robust_list_head = (head != 0).then_some(head);
        tid_address.robust_list_len = len;
    }
    pub fn robust_list(&self) -> Option<(usize, usize)> {
        let tid_address = self.tid_address.lock();
        tid_address
            .robust_list_head
            .map(|head| (head, tid_address.robust_list_len))
    }

    // exec 时关闭线程组中除自身外的其它线程，只清理线程私有状态。
    pub fn close_other_threads_for_exec(&self) {
        let self_tid = self.tid();

        let tasks = self.op_thread_group(|tg| {
            tg.iter()
                .filter(|task| task.tid() != self_tid)
                .collect::<Vec<_>>()
        });

        for task in tasks {
            remove_task(task.tid());
            exit_thread(task, 0);
        }
    }

    /* ======= 操作内部数据 ====== */
    pub fn op_memory_set_read<T>(&self, f: impl FnOnce(&MemorySet) -> T) -> T {
        f(&self.memory_set.read())
    }
    pub fn op_memory_set_write<T>(&self, f: impl FnOnce(&mut MemorySet) -> T) -> T {
        f(&mut self.memory_set.write())
    }
    pub fn op_parent<T>(&self, f: impl FnOnce(&Option<Weak<TaskControlBlock>>) -> T) -> T {
        f(&self.parent.lock())
    }
    pub fn op_children_mut<T>(
        &self,
        f: impl FnOnce(&mut BTreeMap<usize, Arc<TaskControlBlock>>) -> T,
    ) -> T {
        f(&mut self.children.lock())
    }
    // 只读查信号队列
    pub fn op_sig_pending<T>(&self, f: impl FnOnce(&SigPending) -> T) -> T {
        f(&self.sig_pending.lock())
    }

    // 可写改信号队列（加信号、改掩码）
    pub fn op_sig_pending_mut<T>(&self, f: impl FnOnce(&mut SigPending) -> T) -> T {
        f(&mut self.sig_pending.lock())
    }

    // 只读查 handler 表
    pub fn op_sig_handler<T>(&self, f: impl FnOnce(&SigHandler) -> T) -> T {
        f(&self.sig_handler.lock())
    }

    // 可写改 handler 表（sigaction）
    pub fn op_sig_handler_mut<T>(&self, f: impl FnOnce(&mut SigHandler) -> T) -> T {
        f(&mut self.sig_handler.lock())
    }

    // 取信号栈
    pub fn sigstack(&self) -> Option<SignalStack> {
        let stack = *self.sig_stack.lock();
        if stack.ss_flags == (SS_DISABLE as i32) || stack.ss_size == 0 {
            None
        } else {
            Some(stack)
        }
    }

    // 信号入口：给线程发送信号
    // pub fn receive_siginfo(&self, siginfo: SigInfo, thread_level: bool) {
    //     let sig = crate::signal::Sig::from(siginfo.signo);
    //     match thread_level {
    //         true => {
    //             self.op_sig_pending_mut(|pending| pending.add_signal(siginfo));
    //             // SIGKILL/SIGSTOP 必须立即唤醒阻塞的线程，否则信号永远不会被处理
    //             if sig.is_kill_or_stop() && self.is_blocked() {
    //                 crate::task::scheduler::wakeup_task(self.tid());
    //             }
    //         }
    //         false => {
    //             let target = self.op_thread_group(|tg| {
    //                 let mut fallback = None;
    //                 let mut leader = None;
    //                 for task in tg.iter() {
    //                     if fallback.is_none() {
    //                         fallback = Some(task.clone());
    //                     }
    //                     if task.is_process_leader() {
    //                         leader = Some(task.clone());
    //                     }
    //                     let can_take_now = task.op_sig_pending(|pending| {
    //                         !pending.mask.contain_signal(sig) || sig.is_kill_or_stop()
    //                     });
    //                     if can_take_now {
    //                         return Some(task.clone());
    //                     }
    //                 }
    //                 leader.or(fallback)
    //             });
    //             if let Some(task) = target {
    //                 task.op_sig_pending_mut(|pending| pending.add_signal(siginfo));
    //                 // SIGKILL/SIGSTOP 必须立即唤醒阻塞的线程
    //                 if sig.is_kill_or_stop() && task.is_blocked() {
    //                     crate::task::scheduler::wakeup_task(task.tid());
    //                 }
    //             }
    //         }
    //     }
    // }
    fn is_disabled_musl_sigcancel(&self, sig: Sig) -> bool {
        if sig.raw() != 33 {
            return false;
        }

        #[cfg(target_arch = "riscv64")]
        let tp = self.get_trap_cx().x[4];
        #[cfg(target_arch = "loongarch64")]
        let tp = self.get_trap_cx().x[2];
        if tp < 152 {
            return false;
        }

        let mut cancel_state = 0u8;
        copy_from_user(&mut cancel_state as *mut u8, (tp - 152) as *const u8, 1).is_ok()
            && cancel_state == 1
    }

    pub fn receive_siginfo(&self, siginfo: SigInfo, thread_level: bool) {
        let sig = crate::signal::Sig::from(siginfo.signo);

        match thread_level {
            // ===== 线程级信号 =====
            true => {
                self.op_sig_pending_mut(|pending| pending.add_signal(siginfo));

                // ★ 改动：不只是 KILL/STOP 才唤醒，而是调用 check_signal_interrupt
                if self.check_signal_interrupt() && !self.is_disabled_musl_sigcancel(sig) {
                    self.interrupted.store(true, Ordering::Relaxed);
                    if self.is_blocked() {
                        crate::task::scheduler::wakeup_task(self.tid());
                    }
                }
                // 保留原来的 KILL/STOP 立即唤醒逻辑作为兜底
                else if sig.is_kill_or_stop() && self.is_blocked() {
                    self.interrupted.store(true, Ordering::Relaxed);
                    crate::task::scheduler::wakeup_task(self.tid());
                }
            }

            // ===== 进程级信号 =====
            false => {
                // 原来的逻辑：找线程组中第一个能接收的线程
                let target = self.op_thread_group(|tg| {
                    let mut fallback = None;
                    let mut leader = None;
                    for task in tg.iter() {
                        if fallback.is_none() {
                            fallback = Some(task.clone());
                        }
                        if task.is_process_leader() {
                            leader = Some(task.clone());
                        }
                        let can_take_now = task.op_sig_pending(|pending| {
                            !pending.mask.contain_signal(sig) || sig.is_kill_or_stop()
                        });
                        if can_take_now {
                            return Some(task.clone());
                        }
                    }
                    leader.or(fallback)
                });

                if let Some(task) = target {
                    task.op_sig_pending_mut(|pending| pending.add_signal(siginfo));

                    // ★ 改动：同样使用 check_signal_interrupt
                    if task.check_signal_interrupt() && !task.is_disabled_musl_sigcancel(sig) {
                        task.interrupted.store(true, Ordering::Relaxed);
                        if task.is_blocked() {
                            crate::task::scheduler::wakeup_task(task.tid());
                        }
                    } else if sig.is_kill_or_stop() && task.is_blocked() {
                        task.interrupted.store(true, Ordering::Relaxed);
                        crate::task::scheduler::wakeup_task(task.tid());
                    }
                }
            }
        }
    }
    pub fn set_sig_context_addr(&self, addr: usize) {
        self.sig_context_addr.store(addr, Ordering::Relaxed);
    }

    pub fn sig_context_addr(&self) -> usize {
        self.sig_context_addr.load(Ordering::Relaxed)
    }
    pub fn op_thread_group<T>(&self, f: impl FnOnce(&ThreadGroup) -> T) -> T {
        f(&self.thread_group.lock())
    }
    pub fn op_thread_group_mut<T>(&self, f: impl FnOnce(&mut ThreadGroup) -> T) -> T {
        f(&mut self.thread_group.lock())
    }

    // 文件描述符相关操作
    pub fn alloc_fd(&self, fd_entry: FdEntry) -> SysResult<usize> {
        self.fd_table.lock().alloc_fd(fd_entry)
    }
    pub fn alloc_fd_from(&self, fd_entry: FdEntry, min_fd: usize) -> SysResult<usize> {
        self.fd_table.lock().alloc_fd_from(fd_entry, min_fd)
    }
    pub fn set_fd(&self, fd: usize, fd_entry: FdEntry) -> SysResult<Option<FdEntry>> {
        self.fd_table.lock().set_fd(fd, fd_entry)
    }
    pub fn close(&self, fd: usize) -> SysResult {
        self.fd_table.lock().close(fd)
    }
    pub fn get_fd_entry(&self, fd: usize) -> SysResult<FdEntry> {
        self.fd_table.lock().get_fd_entry(fd)
    }
    pub fn nofile_limit(&self) -> (usize, usize) {
        self.fd_table.lock().nofile_limit()
    }
    pub fn set_nofile_limit(&self, cur: usize, max: usize) -> SysResult {
        self.fd_table.lock().set_nofile_limit(cur, max)
    }
}

impl TaskControlBlock {
    // 内核栈操作
    pub fn kstack(&self) -> usize {
        self.kernel_stack.get_top()
    }

    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        let trap_cx_ptr = self.kernel_stack.get_top_edge() - core::mem::size_of::<TrapContext>();
        unsafe { &mut *(trap_cx_ptr as *mut TrapContext) }
    }
    // 克隆异常上下文，注意传入的栈指针应指向栈上异常上下文数据的位置
    fn clone_trap_cx(&self, kernel_stack_ptr: usize) {
        let src_trap_cx_ptr = (self.kernel_stack.get_top_edge()
            - core::mem::size_of::<TrapContext>())
            as *const TrapContext;
        let dst_trap_cx_ptr = kernel_stack_ptr as *mut TrapContext;
        unsafe {
            dst_trap_cx_ptr.write(src_trap_cx_ptr.read());
        }
    }
    // 修改任务上下文，注意传入的栈指针应指向栈上任务上下文数据的位置
    fn write_task_cx(self: &Arc<Self>, kernel_stack_ptr: usize) {
        let token = self.get_user_token();
        let task_cx_ptr = kernel_stack_ptr as *mut TaskContext;
        let task_cx = TaskContext::app_init_task_context(token);
        unsafe {
            task_cx_ptr.write(task_cx);
        }
    }
}

/// 线程退出
///
/// 修改线程退出码，随后移除线程并释放线程占有的资源
fn exit_thread(task: Arc<TaskControlBlock>, exit_code: i32) {
    exit_robust_list(&task);
    if let Some(ctid) = task.clear_child_tid_addr() {
        let zero: i32 = 0;
        let _ = copy_to_user(ctid as *mut i32, &zero as *const i32, 1);
        let _ = crate::task::futex::futex_wake_private(ctid, 1);
        let _ = crate::task::futex::futex_wake(ctid, 1, false);
    }
    // 添加这一行：先从调度器就绪队列中移除
    remove_task(task.tid());
    // 只有数据 线程组 和 TASK_MANAGER 持有对线程的引用，当引用归零时该线程占有的私有资源被释放
    task.op_thread_group_mut(|tg| tg.remove(&task.tid()));
    task.set_exited();
    task.set_exit_code(exit_code);
    TASK_MANAGER.remove(task.tid());
}

fn exit_thread_by_signal(task: Arc<TaskControlBlock>, signal: i32) {
    exit_robust_list(&task);
    if let Some(ctid) = task.clear_child_tid_addr() {
        let zero: i32 = 0;
        let _ = copy_to_user(ctid as *mut i32, &zero as *const i32, 1);
        let _ = crate::task::futex::futex_wake_private(ctid, 1);
        let _ = crate::task::futex::futex_wake(ctid, 1, false);
    }
    remove_task(task.tid());
    task.op_thread_group_mut(|tg| tg.remove(&task.tid()));
    task.set_exited();
    task.set_exit_signal(signal);
    TASK_MANAGER.remove(task.tid());
}

fn exit_robust_list(task: &Arc<TaskControlBlock>) {
    const ROBUST_LIST_HEAD_SIZE: usize = core::mem::size_of::<usize>() * 3;
    const FUTEX_WAITERS: u32 = 0x8000_0000;
    const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
    const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
    const ROBUST_LIST_LIMIT: usize = 2048;

    let Some((head, len)) = task.robust_list() else {
        return;
    };
    if len != ROBUST_LIST_HEAD_SIZE {
        return;
    }

    let mut first_entry = 0usize;
    let mut futex_offset = 0isize;
    let mut pending = 0usize;
    if copy_from_user(&mut first_entry as *mut usize, head as *const usize, 1).is_err()
        || copy_from_user(
            &mut futex_offset as *mut isize,
            (head + core::mem::size_of::<usize>()) as *const isize,
            1,
        )
        .is_err()
        || copy_from_user(
            &mut pending as *mut usize,
            (head + core::mem::size_of::<usize>() * 2) as *const usize,
            1,
        )
        .is_err()
    {
        return;
    }

    let mut entry = first_entry;
    for _ in 0..ROBUST_LIST_LIMIT {
        if entry == 0 || entry == head {
            break;
        }
        handle_robust_entry(
            task.tid(),
            entry,
            futex_offset,
            FUTEX_WAITERS,
            FUTEX_OWNER_DIED,
            FUTEX_TID_MASK,
        );

        let mut next = 0usize;
        if copy_from_user(&mut next as *mut usize, entry as *const usize, 1).is_err() {
            break;
        }
        entry = next;
    }

    if pending != 0 {
        handle_robust_entry(
            task.tid(),
            pending,
            futex_offset,
            FUTEX_WAITERS,
            FUTEX_OWNER_DIED,
            FUTEX_TID_MASK,
        );
    }
}

fn handle_robust_entry(
    tid: usize,
    entry: usize,
    futex_offset: isize,
    futex_waiters: u32,
    futex_owner_died: u32,
    futex_tid_mask: u32,
) {
    let Some(futex_addr) = entry.checked_add_signed(futex_offset) else {
        return;
    };

    let mut value = 0u32;
    if copy_from_user(&mut value as *mut u32, futex_addr as *const u32, 1).is_err() {
        return;
    }
    if value & futex_tid_mask != tid as u32 {
        return;
    }

    let new_value = (value & futex_waiters) | futex_owner_died;
    let _ = copy_to_user(futex_addr as *mut u32, &new_value as *const u32, 1);
    let _ = crate::task::futex::futex_wake(futex_addr, 1, true);
    let _ = crate::task::futex::futex_wake(futex_addr, 1, false);
}

/// 线程退出 - 对外接口
///
/// - 设置当前线程退出状态
/// - 处理 clear_child_tid
/// - 释放线程私有资源
/// - 从线程组中移除自己
/// - 最后一个线程则进入进程级退出流程（目前未实现）
/// TODO: 当前退出线程组主线程时会退出整个线程组，与 Linux 实现存在差异
pub fn task_exit(task: Arc<TaskControlBlock>, exit_code: i32) {
    // warn! {"[kernel] Thread exit. tid: {}, tgid: {}, thread_count: {}.", task.tid(), task.tgid(), task.op_thread_group(|tg| tg.iter().count())}

    if task.is_process_leader() {
        task_group_exit(task, exit_code);
        return;
    } else {
        exit_thread(task, exit_code);
    }
}

pub fn task_exit_by_signal(task: Arc<TaskControlBlock>, signal: i32) {
    task_group_exit_by_signal(task, signal);
}

/// 进程退出
///
/// - 杀掉/停止线程组内所有线程
/// - 关闭文件描述符
/// - 释放地址空间
/// - 释放信号处理、文件系统上下文等共享资源
/// - 给父进程发送 SIGCHLD
/// - 留下 exited 状态，等待父进程 wait 回收
pub fn task_group_exit(task: Arc<TaskControlBlock>, exit_code: i32) {
    let tgid = task.tgid();
    let threads = task.op_thread_group(|tg| tg.iter().collect::<Vec<_>>());
    let leader = threads
        .iter()
        .find(|thread| thread.tid() == tgid)
        .cloned()
        .unwrap_or_else(|| task.clone());

    for thread in threads {
        remove_task(thread.tid());
        if thread.tid() != leader.tid() {
            exit_thread(thread, exit_code);
        }
    }

    leader.op_thread_group_mut(|tg| tg.remove(&leader.tid()));

    // 修改孩子进程的父亲——托孤。children 是进程级资源，只处理一次。
    let children = task.op_children_mut(core::mem::take);
    for (_, child) in children {
        child.set_parent(&INITPROC);
        INITPROC.add_child(child);
    }

    // 回收进程级共享资源。
    task.op_memory_set_write(|mem| {
        mem.recycle_data_pages();
    });
    task.fd_table.lock().clear();

    exit_robust_list(&leader);
    leader.set_exited();
    leader.set_exit_code(exit_code);
    // 向父进程发送 SIGCHLD 信号
    leader.op_parent(|parent_opt| {
        if let Some(parent) = parent_opt.as_ref().and_then(|w| w.upgrade()) {
            let siginfo = SigInfo::new(
                Sig::SIGCHLD.raw(),
                SigInfo::CLD_EXITED,
                SiField::Kill { tid: leader.tid() },
            );
            parent.receive_siginfo(siginfo, false);
        }
    });

    // 清空残留信号
    leader.op_sig_pending_mut(|p| p.clear());

    TASK_MANAGER.remove(leader.tid());
}

pub fn task_group_exit_by_signal(task: Arc<TaskControlBlock>, signal: i32) {
    let tgid = task.tgid();
    let threads = task.op_thread_group(|tg| tg.iter().collect::<Vec<_>>());
    let leader = threads
        .iter()
        .find(|thread| thread.tid() == tgid)
        .cloned()
        .unwrap_or_else(|| task.clone());

    for thread in threads {
        remove_task(thread.tid());
        if thread.tid() != leader.tid() {
            exit_thread_by_signal(thread, signal);
        }
    }

    leader.op_thread_group_mut(|tg| tg.remove(&leader.tid()));

    let children = task.op_children_mut(core::mem::take);
    for (_, child) in children {
        child.set_parent(&INITPROC);
        INITPROC.add_child(child);
    }

    task.op_memory_set_write(|mem| {
        mem.recycle_data_pages();
    });
    task.fd_table.lock().clear();

    exit_robust_list(&leader);
    leader.set_exited();
    leader.set_exit_signal(signal);
    leader.op_parent(|parent_opt| {
        if let Some(parent) = parent_opt.as_ref().and_then(|w| w.upgrade()) {
            let siginfo = SigInfo::new(
                Sig::SIGCHLD.raw(),
                SigInfo::CLD_KILLED,
                SiField::Kill { tid: leader.tid() },
            );
            parent.receive_siginfo(siginfo, false);
        }
    });

    leader.op_sig_pending_mut(|p| p.clear());

    TASK_MANAGER.remove(leader.tid());
}

// 将命令行参数和环境变量压入用户栈
fn init_user_stack(
    memory_set: &mut MemorySet,
    args_vec: &[String],
    envs_vec: &[String],
    mut auxv: Vec<AuxHeader>,
    user_sp: &mut usize,
) -> SysResult<(usize, usize, usize, usize)> {
    const STACK_ALIGN: usize = 16;

    #[inline(always)]
    fn align_down(addr: usize) -> usize {
        addr & !(STACK_ALIGN - 1)
    }

    fn push_bytes_to_stack(
        memory_set: &mut MemorySet,
        bytes: &[u8],
        stack_ptr: &mut usize,
    ) -> SysResult<usize> {
        *stack_ptr -= bytes.len();
        memory_set.write_bytes_to_mapped_range(*stack_ptr, bytes)?;
        Ok(*stack_ptr)
    }

    fn push_strings_to_stack(
        memory_set: &mut MemorySet,
        strings: &[String],
        stack_ptr: &mut usize,
    ) -> SysResult<Vec<usize>> {
        let mut addresses = Vec::with_capacity(strings.len());

        for string in strings {
            let addr = push_bytes_to_stack(memory_set, &[0], stack_ptr)?;
            *stack_ptr = addr - string.len();
            memory_set.write_bytes_to_mapped_range(*stack_ptr, string.as_bytes())?;
            addresses.push(*stack_ptr);
        }

        *stack_ptr = align_down(*stack_ptr);
        Ok(addresses)
    }

    fn push_usize_to_stack(
        memory_set: &mut MemorySet,
        value: usize,
        stack_ptr: &mut usize,
    ) -> SysResult {
        *stack_ptr -= core::mem::size_of::<usize>();
        memory_set.write_bytes_to_mapped_range(*stack_ptr, &value.to_ne_bytes())
    }

    fn push_pointers_to_stack(
        memory_set: &mut MemorySet,
        pointers: &[usize],
        stack_ptr: &mut usize,
    ) -> SysResult<usize> {
        push_usize_to_stack(memory_set, 0, stack_ptr)?;
        for &ptr in pointers.iter().rev() {
            push_usize_to_stack(memory_set, ptr, stack_ptr)?;
        }
        Ok(*stack_ptr)
    }

    fn push_auxv_to_stack(
        memory_set: &mut MemorySet,
        auxv: &[AuxHeader],
        stack_ptr: &mut usize,
    ) -> SysResult<usize> {
        for header in auxv.iter().rev() {
            push_usize_to_stack(memory_set, header.value, stack_ptr)?;
            push_usize_to_stack(memory_set, header.aux_type, stack_ptr)?;
        }
        Ok(*stack_ptr)
    }

    *user_sp = align_down(*user_sp);

    // 字符串内容可以位于指针数组之上，argv/envp 数组保存实际地址。
    let envp = push_strings_to_stack(memory_set, envs_vec, user_sp)?;
    let argv = push_strings_to_stack(memory_set, args_vec, user_sp)?;

    // —— 压入 AT_PLATFORM 字符串 ——
    #[cfg(target_arch = "riscv64")]
    let platform: &str = "RISC-V64";
    #[cfg(target_arch = "loongarch64")]
    let platform: &str = "loongarch64";

    *user_sp -= platform.len() + 1;
    *user_sp -= *user_sp % core::mem::size_of::<usize>();
    memory_set.write_bytes_to_mapped_range(*user_sp, platform.as_bytes())?;
    memory_set.write_bytes_to_mapped_range(*user_sp + platform.len(), &[0])?;
    let platform_addr = *user_sp;

    // —— 压入 16 字节随机数 ——
    *user_sp -= 16;
    let random_addr = *user_sp;
    let mut random = [0u8; 16];
    let mut seed = random_addr ^ args_vec.len() ^ envs_vec.len().wrapping_shl(8);
    for (idx, byte) in random.iter_mut().enumerate() {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed ^= (idx + 1usize).wrapping_mul(0x9e37_79b9);
        *byte = seed as u8;
    }
    memory_set.write_bytes_to_mapped_range(random_addr, &random)?;

    // —— 追加动态 aux 条目 ——
    auxv.push(AuxHeader {
        aux_type: AT_PLATFORM,
        value: platform_addr,
    });
    auxv.push(AuxHeader {
        aux_type: AT_RANDOM,
        value: random_addr,
    });
    auxv.push(AuxHeader {
        aux_type: AT_EXECFN,
        value: argv[0],
    });
    auxv.push(AuxHeader {
        aux_type: AT_NULL,
        value: 0,
    });

    // 预留填充空间，使压入 argc/argv/envp/auxv 后的最终 sp 仍保持 16 字节对齐。
    let pointer_count = 1                          // argc
        + argv.len() + 1                           // argv[] + NULL
        + envp.len() + 1                           // envp[] + NULL
        + auxv.len() * 2; // auxv (type + value pairs)
    let pointer_bytes = pointer_count * core::mem::size_of::<usize>();
    let padding = (STACK_ALIGN - pointer_bytes % STACK_ALIGN) % STACK_ALIGN;
    *user_sp -= padding;

    let auxv_base = push_auxv_to_stack(memory_set, &auxv, user_sp)?;
    let envp_base = push_pointers_to_stack(memory_set, &envp, user_sp)?;
    let argv_base = push_pointers_to_stack(memory_set, &argv, user_sp)?;
    push_usize_to_stack(memory_set, args_vec.len(), user_sp)?;

    Ok((argv_base, envp_base, auxv_base, *user_sp))
}

/// 线程组结构
pub struct ThreadGroup {
    member: BTreeMap<usize, Weak<TaskControlBlock>>,
}

impl ThreadGroup {
    pub fn new() -> Self {
        Self {
            member: BTreeMap::new(),
        }
    }
    pub fn size(&self) -> usize {
        self.member.len()
    }

    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.member.insert(task.tid(), Arc::downgrade(&task));
    }
    pub fn remove(&mut self, tid: &usize) {
        self.member.remove(tid);
    }

    pub fn iter(&self) -> impl Iterator<Item = Arc<TaskControlBlock>> + '_ {
        self.member.values().filter_map(|task| task.upgrade())
    }
}

/// 任务状态
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    Ready,   // 已就绪
    Running, // 正在运行
    Blocked, // 阻塞
    Exited,  // 已退出
}

bitflags! {
    /// clone/fork 系统调用使用的标志位。
    ///
    /// Linux 的 clone 参数低 8 位不是普通的共享标志，而是子任务退出时
    /// 发送给父任务的信号编号；真正的 `CLONE_*` 标志从 bit 8 开始。
    pub struct CloneFlags: u32 {
        /// 退出信号掩码，低 8 位用于保存子任务退出时发送的信号编号。
        const EXIT_SIGNAL_MASK = 0xff;
        /// 子任务退出时向父任务发送 SIGCHLD。数值为 17，即退出信号编号。
        const SIGCHLD = 17;

        /// 共享地址空间；父子任务看到同一组用户虚拟内存映射。
        const CLONE_VM = 1 << 8;
        /// 共享文件系统上下文，例如当前工作目录和根目录。
        const CLONE_FS = 1 << 9;
        /// 共享文件描述符表；一方打开、关闭或替换 fd 会影响另一方。
        const CLONE_FILES = 1 << 10;
        /// 共享信号处理函数表。Linux 要求同时设置 `CLONE_VM`。
        const CLONE_SIGHAND = 1 << 11;
        /// 在父任务指定地址写入子任务 pidfd。
        const CLONE_PIDFD = 1 << 12;
        /// 子任务继续处于被 ptrace 跟踪状态。
        const CLONE_PTRACE = 1 << 13;
        /// 父任务阻塞到子任务 exec 或 exit；通常配合 vfork 语义使用。
        const CLONE_VFORK = 1 << 14;
        /// 子任务的父任务设为调用者的父任务，而不是调用者本身。
        const CLONE_PARENT = 1 << 15;
        /// 创建同一线程组内的新线程。Linux 要求同时设置 `CLONE_SIGHAND` 和 `CLONE_VM`。
        const CLONE_THREAD = 1 << 16;
        /// 为子任务创建新的 mount namespace。
        const CLONE_NEWNS = 1 << 17;
        /// 共享 System V semaphore undo 状态。
        const CLONE_SYSVSEM = 1 << 18;
        /// 设置子任务 TLS 指针。
        const CLONE_SETTLS = 1 << 19;
        /// 在父任务指定地址写入子任务 tid。
        const CLONE_PARENT_SETTID = 1 << 20;
        /// 子任务退出时清零指定地址并唤醒 futex 等待者。
        const CLONE_CHILD_CLEARTID = 1 << 21;
        /// 历史遗留标志，现代 Linux 基本忽略。
        const CLONE_DETACHED = 1 << 22;
        /// 阻止跟踪器强制对子任务设置 `CLONE_PTRACE`。
        const CLONE_UNTRACED = 1 << 23;
        /// 在子任务指定地址写入自己的 tid。
        const CLONE_CHILD_SETTID = 1 << 24;
        /// 为子任务创建新的 cgroup namespace。
        const CLONE_NEWCGROUP = 1 << 25;
        /// 为子任务创建新的 UTS namespace。
        const CLONE_NEWUTS = 1 << 26;
        /// 为子任务创建新的 IPC namespace。
        const CLONE_NEWIPC = 1 << 27;
        /// 为子任务创建新的 user namespace。
        const CLONE_NEWUSER = 1 << 28;
        /// 为子任务创建新的 PID namespace。
        const CLONE_NEWPID = 1 << 29;
        /// 为子任务创建新的 network namespace。
        const CLONE_NEWNET = 1 << 30;
        /// 共享 I/O 上下文。
        const CLONE_IO = 1 << 31;
    }
}

impl CloneFlags {
    /// 返回 clone 参数低 8 位携带的退出信号编号。
    pub fn exit_signal(self) -> u32 {
        self.bits() & Self::EXIT_SIGNAL_MASK.bits()
    }

    /// 返回除退出信号之外的 `CLONE_*` 共享/命名空间标志。
    pub fn clone_flags(self) -> Self {
        self & !Self::EXIT_SIGNAL_MASK
    }
}
