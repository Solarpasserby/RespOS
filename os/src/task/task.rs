// os/src/task/task.rs

use spin::{Mutex, MutexGuard};
use alloc::vec::Vec;
use alloc::sync::{Arc, Weak};
use crate::trap::TrapContext;
use crate::mm::MemorySet;
use crate::fs::{FdTable, FdEntry, Path, vfs::ROOT_DENTRY};
use crate::syscall::SysResult;
use super::context::TaskContext;
use super::pid::{PidHandle, pid_alloc};
use super::kstack::KernelStack;


/// 任务控制块——此处的任务是对一定资源和某个程序的抽象表述
/// 
/// - 功能：将程序的一次执行过程及其使用的硬件资源抽象，从而方便内核调度（仅基于个人理解）
/// - 内容：
///     - `pid`          ~~进程~~任务号
///     - `kernel_stack` 任务内核栈
///     - `inner`        任务控制块内部数据
pub struct TaskControlBlock {
    // 不可变
    pub pid: PidHandle,
    pub kernel_stack: KernelStack,
    // 可变
    inner: Mutex<TaskControlBlockInner>,
}

impl TaskControlBlock {
    /// 新建任务
    /// 
    /// 事实上只有初始任务会借由这个方法产生
    pub fn new(elf_data: &[u8]) -> Self {
        let (memory_set, token, user_sp, entry_point) = MemorySet::from_elf_data(elf_data);
        let pid = pid_alloc();
        let kernel_stack = KernelStack::new(&pid);
        let mut kernel_stack_top = kernel_stack.get_top();
        // 初始化内核栈上的异常上下文，并移动栈顶指针
        let trap_context = TrapContext::init_app_context(entry_point, user_sp);
        kernel_stack_top -= core::mem::size_of::<TrapContext>();
        let trap_cx_ptr = kernel_stack_top as *mut TrapContext;
        
        // 创建进程控制块
        let task_ctrl_block = Self {
            pid,
            kernel_stack,
            inner: Mutex::new(TaskControlBlockInner{
                task_status: TaskStatus::Ready,
                task_context: TaskContext::app_init_task_context(kernel_stack_top, token),
                memory_set,
                fd_table: FdTable::new(),
                cwd: Path::new(ROOT_DENTRY.clone()),
                parent: None,
                children: Vec::new(),
                base_size: user_sp,
                exit_code: 0,
            }),
        };
        unsafe { trap_cx_ptr.write(trap_context); }
    
        task_ctrl_block
    }

    /// 以父~~进程~~任务创建子~~进程~~任务
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        let mut parent_inner = self.inner_exclusive_access();
        let pid = pid_alloc();
        let kernel_stack = KernelStack::new(&pid);
        let mut kernel_stack_top = kernel_stack.get_top();
        // 克隆内核栈上的异常上下文
        self.clone_trap_cx(kernel_stack_top);
        kernel_stack_top -= core::mem::size_of::<TrapContext>();

        let task_ctrl_block = Arc::new(TaskControlBlock {
            pid,
            kernel_stack,
            inner: Mutex::new(TaskControlBlockInner {
                task_status: TaskStatus::Ready,
                // 全零初始化任务上下文
                task_context: TaskContext::app_init_task_context(0,0),
                memory_set: MemorySet::from_existed_user(&parent_inner.memory_set),
                fd_table: FdTable::from_existed_user(&parent_inner.fd_table),
                cwd: Path::from_existed_user(&parent_inner.cwd),
                parent: Some(Arc::downgrade(self)),
                children: Vec::new(),
                base_size: parent_inner.base_size,
                exit_code: 0,
            }),
        });
        // 修改任务异常上下文
        task_ctrl_block.write_task_cx(kernel_stack_top);

        // 同时更新父任务状态
        parent_inner.children.push(task_ctrl_block.clone());

        task_ctrl_block
    }

    // 为任务载入可执行程序
    pub fn exec(&self, elf_data: &[u8]) {
        // 主要修改任务的地址空间和异常上下文
        let (memory_set, _token, user_sp, entry_point) = MemorySet::from_elf_data(elf_data);
        let mut inner = self.inner_exclusive_access();
        inner.memory_set = memory_set;
        let trap_cx = self.get_trap_cx();
        *trap_cx = TrapContext::init_app_context(
            entry_point,
            user_sp,
        );
    }

    pub fn inner_exclusive_access(&self) -> MutexGuard<'_, TaskControlBlockInner> {
        self.inner.lock()
    }

    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        let trap_cx_ptr = self.kernel_stack.get_top() - core::mem::size_of::<TrapContext>();
        unsafe { &mut *(trap_cx_ptr as *mut TrapContext) }
    }
    fn clone_trap_cx(&self, dst_kstack_top: usize) {
        let src_trap_cx_ptr =
            (self.kernel_stack.get_top() - core::mem::size_of::<TrapContext>()) as *const TrapContext;
        let dst_trap_cx_ptr = 
            (dst_kstack_top - core::mem::size_of::<TrapContext>()) as *mut TrapContext;
        unsafe { dst_trap_cx_ptr.write(src_trap_cx_ptr.read()); }
    }
    fn write_task_cx(&self, kernel_stack_ptr: usize) {
        let mut inner = self.inner_exclusive_access();
        let token = inner.get_user_token();
        let task_context = &mut inner.task_context;
        task_context.set_sp(kernel_stack_ptr);
        task_context.set_satp(token);
    }

    pub fn pid(&self) -> usize {
        self.pid.0
    }

    // 文件描述符相关操作
    pub fn alloc_fd(&self, fd_entry: FdEntry) -> SysResult<usize> {
        self.inner_exclusive_access().fd_table.alloc_fd(fd_entry)
    }
    pub fn set_fd(&self, fd: usize, fd_entry: FdEntry) -> SysResult<Option<FdEntry>> {
        self.inner_exclusive_access().fd_table.set_fd(fd, fd_entry)
    }
    pub fn get_fd_entry(&self, fd: usize) -> SysResult<FdEntry> {
        self.inner_exclusive_access().fd_table.get_fd_entry(fd)
    }

    // 获取当前工作路径
    pub fn cwd(&self) -> Arc<Path> {
        self.inner_exclusive_access().cwd.clone()
    }
    pub fn set_cwd(&self, path: Arc<Path>) {
        self.inner_exclusive_access().cwd = path; 
    }
}

/// 任务控制块内部数据
/// 
/// - 功能：用于保存任务的执行状态、地址空间、父子任务关系等信息
/// - 内容:
///     - `task_status`   任务状态
///     - `task_context`  任务上下文
///     - `memory_set`    任务的地址空间
///     - `fd_table`      任务的文件描述符表，记录了当前任务使用的文件描述符
///     - `cwd`           任务的当前工作目录
///     - ~~`trap_cx_ppn` 任务的异常上下文所在物理页号~~ 原字段只是为了获取页帧上的数据，现在使用更~~安全~~高效的方法获取
///     - `parent`        任务的父任务（进程），使用弱引用避免环形引用
///     - `children`      任务的子任务（进程），使用原子计数引用
///     - `base_size`     任务的内存空间大小 -> 暂时不知道有啥用，这里直接取栈顶的数值
///     - `exit_code`     任务退出码
pub struct TaskControlBlockInner {
    pub task_status: TaskStatus,
    pub task_context: TaskContext,
    pub memory_set: MemorySet,
    pub fd_table: FdTable,
    pub cwd: Arc<Path>,
    pub parent: Option<Weak<TaskControlBlock>>,
    pub children: Vec<Arc<TaskControlBlock>>,
    pub base_size: usize,
    pub exit_code: i32,
}

impl TaskControlBlockInner {
    /// 获取用户任务页表的 `stap` 寄存器值
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    fn get_status(&self) -> TaskStatus {
        self.task_status
    }
    pub fn is_zombie(&self) -> bool {
        self.get_status() == TaskStatus::Exited
    }
}

/// 任务状态
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    Ready,   // 已就绪
    Running, // 正在运行
    Exited,  // 已退出
}