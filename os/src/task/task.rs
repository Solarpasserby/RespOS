// os/src/task/task.rs
use super::INITPROC;
use super::context::TaskContext;
use super::kstack::KernelStack;
use super::manager::TASK_MANAGER;
use super::scheduler::remove_task;
use super::tid::{TidHandle, tid_alloc};
use crate::fs::{FdEntry, FdTable, Path, vfs::ROOT_DENTRY};
use crate::mm::MemorySet;
use crate::mutex::SpinLock;
use crate::signal::sig_handler::SigHandler;
use crate::signal::sig_info::SigInfo;
use crate::signal::sig_stack::SignalStack;
use crate::signal::sig_struct::SigPending;
use crate::syscall::{Errno, SysResult};
use crate::trap::TrapContext;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use bitflags::bitflags;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use spin::RwLock;

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
    // task_context: TaskContext, // 注意任务上下文的处理

    // 内存管理
    memory_set: Arc<RwLock<MemorySet>>,

    // 文件系统
    fd_table: SpinLock<Arc<FdTable>>,
    cwd: Arc<SpinLock<Arc<Path>>>,

    //信号
    sig_pending: SpinLock<SigPending>, // 本线程的信号队列 + 掩码（独享）
    sig_stack: SpinLock<SignalStack>,  // 本线程的备用信号栈（独享）
    sig_handler: Arc<SpinLock<SigHandler>>, // 线程组共享的 handler 注册表（共享）
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
            // task_context: TaskContext, // 注意任务上下文的处理

            // 内存管理
            memory_set: Arc::new(RwLock::new(MemorySet::new())),

            // 文件系统
            fd_table: SpinLock::new(FdTable::new()),
            cwd: Arc::new(SpinLock::new(Path::new(ROOT_DENTRY.clone()))),

            //信号
            sig_pending: SpinLock::new(SigPending::new()),
            sig_stack: SpinLock::new(SignalStack::default()),
            sig_handler: Arc::new(SpinLock::new(SigHandler::new())),
        }
    }

    /// 新建任务
    ///
    /// 事实上只有初始任务会借由这个方法产生
    pub fn init(elf_data: &[u8]) -> Arc<Self> {
        let tid: TidHandle = tid_alloc();
        let tgid = tid.0;
        // 创建地址空间会拷贝内核页表，先创建内核栈生成页表映射，以保证任务切换后能正确访问内核栈
        let mut kernel_stack = KernelStack::new(&tid);
        let (memory_set, token, user_sp, entry_point) = MemorySet::from_elf_data(elf_data);

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

            // 内存管理
            memory_set: Arc::new(RwLock::new(memory_set)),

            // 文件系统
            fd_table: SpinLock::new(FdTable::new()),
            cwd: Arc::new(SpinLock::new(Path::new(ROOT_DENTRY.clone()))),

            //信号
            sig_pending: SpinLock::new(SigPending::new()),
            sig_stack: SpinLock::new(SignalStack::default()),
            sig_handler: Arc::new(SpinLock::new(SigHandler::new())),
        });

        // 在线程组中添加该线程
        task_ctrl_block
            .thread_group
            .lock()
            .add(task_ctrl_block.clone());

        // 生成 tp 指针
        let task_ptr = Arc::as_ptr(&task_ctrl_block) as usize;
        // 初始化内核栈上的异常上下文
        let mut trap_context = TrapContext::init_app_context(entry_point, user_sp, 0, 0, 0, 0);
        trap_context.set_tp(task_ptr);
        // 初始化任务上下文
        let task_context = TaskContext::app_init_task_context(task_ptr, token);

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
    pub fn fork(self: &Arc<Self>, flags: CloneFlags) -> Arc<Self> {
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
        let (tgid, thread_group, parent, children, cwd) = if is_thread {
            // 创建线程，属于同一线程组
            (
                self.tgid(),
                self.thread_group.clone(),
                self.parent.clone(),
                self.children.clone(),
                self.cwd.clone(),
            )
        } else {
            // 创建进程
            (
                tid.0,
                Arc::new(SpinLock::new(ThreadGroup::new())),
                Arc::new(SpinLock::new(Some(Arc::downgrade(&process_leader)))),
                Arc::new(SpinLock::new(BTreeMap::new())),
                Arc::new(SpinLock::new(Path::from_existed_user(&self.cwd()))),
            )
        };

        // 是否与父线程共享地址空间
        let memory_set = if flags.contains(CloneFlags::CLONE_VM) {
            // TODO: exec 替换地址空间时可能会出现问题
            self.memory_set.clone()
        } else {
            Arc::new(RwLock::new(MemorySet::from_existed_user(
                &self.memory_set.read(),
            )))
        };

        // 是否与父线程共享文件数据
        let fd_table = if is_thread || flags.contains(CloneFlags::CLONE_FILES) {
            self.fd_table.lock().clone()
        } else {
            FdTable::from_existed_user(&self.fd_table.lock())
        };
        let (sig_handler, sig_pending, sig_stack) = if is_thread {
            (
                self.sig_handler.clone(),              // 共享同一张 handler 表
                SpinLock::new(SigPending::new()),      // 自己的队列
                SpinLock::new(SignalStack::default()), // 自己的栈
            )
        } else {
            (
                Arc::new(SpinLock::new(SigHandler::new())), // 全新的表
                SpinLock::new(SigPending::new()),
                SpinLock::new(SignalStack::default()),
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

            // 内存管理
            memory_set,

            // 文件系统
            fd_table: SpinLock::new(fd_table),
            cwd,

            // 信号
            sig_pending,
            sig_stack,
            sig_handler,
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
        elf_data: &[u8],
        args: Vec<String>,
        envs: Vec<String>,
    ) -> SysResult<usize> {
        // 简化模型：只有进程 leader 可以 exec，避免非 leader exec 后父子关系和 tgid 语义混乱。
        if !self.is_process_leader() {
            return Err(Errno::EINVAL);
        }

        let (memory_set, _token, mut user_sp, entry_point) = MemorySet::from_elf_data(elf_data);

        /* ===== 修改地址空间 ===== */
        let mut memory_set_guard = self.memory_set.write();
        let old_memory_set = core::mem::replace(&mut *memory_set_guard, memory_set);
        // 刷新页表，由于应用程序通过异常进入，在异常返回时不会刷新页表
        // 为了程序返回后看到的地址空间为自身而非父任务的地址空间，需要主动刷新页表
        memory_set_guard.activate();
        drop(old_memory_set);
        drop(memory_set_guard);

        /* ===== 修改用户栈数据 ===== */
        // 需保证页表已刷新，函数内部直接访存高度依赖
        let (argv_base, envp_base, stack_top) =
            init_user_stack(args.as_slice(), envs.as_slice(), &mut user_sp);

        /* ===== 修改异常上下文 ===== */
        let argc = args.len();
        let trap_cx = self.get_trap_cx();
        *trap_cx =
            TrapContext::init_app_context(entry_point, stack_top, argc, argv_base, envp_base, 0);
        trap_cx.set_tp(Arc::as_ptr(&self) as usize);

        /* ===== 修改线程组 ===== */
        self.close_other_threads_for_exec();

        /* ===== 修改文件描述符表 ===== */
        // exec 保留 fd_table；后续可在这里处理 close-on-exec。

        /* ===== 修改信号处理 ===== */
        // TODO: 信号完善

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
    pub fn exit_code(&self) -> i32 {
        self.exit_code.load(Ordering::Relaxed)
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
    pub fn set_exit_code(&self, exit_code: i32) {
        self.exit_code.swap(exit_code, Ordering::Relaxed);
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
        if stack.ss_flags == 1 {
            None
        } else {
            Some(stack)
        }
    }

    // 信号入口：给线程发送信号
    pub fn receive_siginfo(&self, siginfo: SigInfo, thread_level: bool) {
        match thread_level {
            true => {
                self.op_sig_pending_mut(|pending| pending.add_signal(siginfo));
            }
            false => {
                self.op_thread_group(|tg| {
                    for task in tg.iter() {
                        task.op_sig_pending_mut(|pending| pending.add_signal(siginfo));
                    }
                });
            }
        }
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
    pub fn set_fd(&self, fd: usize, fd_entry: FdEntry) -> SysResult<Option<FdEntry>> {
        self.fd_table.lock().set_fd(fd, fd_entry)
    }
    pub fn close(&self, fd: usize) -> SysResult {
        self.fd_table.lock().close(fd)
    }
    pub fn get_fd_entry(&self, fd: usize) -> SysResult<FdEntry> {
        self.fd_table.lock().get_fd_entry(fd)
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
        let task_cx = TaskContext::app_init_task_context(Arc::as_ptr(self) as usize, token);
        unsafe {
            task_cx_ptr.write(task_cx);
        }
    }
}

fn exit_thread(task: Arc<TaskControlBlock>, exit_code: i32) {
    task.op_thread_group_mut(|tg| tg.remove(&task.tid()));
    task.set_exited();
    task.set_exit_code(exit_code);
    TASK_MANAGER.remove(task.tid());
}

/// 进程退出。
///
/// 当前简化模型中，sys_exit 退出整个线程组；只有进程 leader 会留在父进程
/// children 中等待 wait4 回收，普通线程不会作为子进程暴露给父进程。
pub fn task_exit(task: Arc<TaskControlBlock>, exit_code: i32) {
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

    leader.set_exited();
    leader.set_exit_code(exit_code);
    // TODO: 向父进程发送 SIGCHLD 信号
    // TODO: 清空信号

    TASK_MANAGER.remove(leader.tid());
}

// 将命令行参数和环境变量压入用户栈
fn init_user_stack(
    args_vec: &[String],
    envs_vec: &[String],
    user_sp: &mut usize,
) -> (usize, usize, usize) {
    const STACK_ALIGN: usize = 16;

    #[inline(always)]
    fn align_down(addr: usize) -> usize {
        addr & !(STACK_ALIGN - 1)
    }

    fn push_strings_to_stack(strings: &[String], stack_ptr: &mut usize) -> Vec<usize> {
        let mut addresses = Vec::with_capacity(strings.len());

        for string in strings {
            *stack_ptr -= string.len() + 1;
            let ptr = *stack_ptr as *mut u8;
            unsafe {
                ptr.copy_from_nonoverlapping(string.as_ptr(), string.len());
                ptr.add(string.len()).write(0);
            }
            addresses.push(*stack_ptr);
        }

        *stack_ptr = align_down(*stack_ptr);
        addresses
    }

    fn push_usize_to_stack(value: usize, stack_ptr: &mut usize) {
        *stack_ptr -= core::mem::size_of::<usize>();
        unsafe {
            *(*stack_ptr as *mut usize) = value;
        }
    }

    fn push_pointers_to_stack(pointers: &[usize], stack_ptr: &mut usize) -> usize {
        push_usize_to_stack(0, stack_ptr);
        for &ptr in pointers.iter().rev() {
            push_usize_to_stack(ptr, stack_ptr);
        }
        *stack_ptr
    }

    *user_sp = align_down(*user_sp);

    // 字符串内容可以位于指针数组之上，argv/envp 数组保存实际地址。
    let envp = push_strings_to_stack(envs_vec, user_sp);
    let argv = push_strings_to_stack(args_vec, user_sp);

    // 预留 padding，使压入 argc/argv/envp 后的最终 sp 仍保持 16 字节对齐。
    let pointer_count = 1 + argv.len() + 1 + envp.len() + 1;
    let pointer_bytes = pointer_count * core::mem::size_of::<usize>();
    let padding = (STACK_ALIGN - pointer_bytes % STACK_ALIGN) % STACK_ALIGN;
    *user_sp -= padding;

    let envp_base = push_pointers_to_stack(&envp, user_sp);
    let argv_base = push_pointers_to_stack(&argv, user_sp);
    push_usize_to_stack(args_vec.len(), user_sp);

    (argv_base, envp_base, *user_sp)
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
