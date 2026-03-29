// os/src/task/task.rs

use alloc::vec::Vec;
use alloc::sync::{ Arc, Weak };
use core::cell::RefMut;
use crate::config::TRAP_CONTEXT;
use crate::sync::UPSafeCell;
use crate::trap::{ TrapContext, trap_handler };
use crate::mm::{ KERNEL_SPACE, MemorySet, VirtAddr, PhysAddr };
use super::context::TaskContext;
use super::pid::{ KernelStack, PidHandle, pid_alloc };


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
    inner: UPSafeCell<TaskControlBlockInner>,
}

impl TaskControlBlock {
    pub fn new(elf_data: &[u8]) -> Self {
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf_data(elf_data);
        let pid = pid_alloc();
        let kernel_stack = KernelStack::new(&pid);
        let kernel_stack_top = kernel_stack.get_top();
        let task_ctrl_block = Self {
            pid,
            kernel_stack,
            inner: unsafe { UPSafeCell::new(TaskControlBlockInner{
                task_status: TaskStatus::Ready,
                task_context: TaskContext::create_for_kstack_restore(kernel_stack_top),
                memory_set,
                parent: None,
                children: Vec::new(),
                base_size: user_sp,
                exit_code: 0,
            }) }
        };
        let trap_cx = task_ctrl_block.inner_exclusive_access().get_trap_cx();
        *trap_cx = TrapContext::init_app_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as *const() as usize
        );

        task_ctrl_block
    }

    /// 以父~~进程~~任务创建子~~进程~~任务
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        let mut parent_inner = self.inner_exclusive_access();
        let pid = pid_alloc();
        let kernel_stack = KernelStack::new(&pid);
        let kernel_stack_top = kernel_stack.get_top();
        let task_ctrl_block = Arc::new(TaskControlBlock {
            pid,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    task_status: TaskStatus::Ready,
                    task_context: TaskContext::create_for_kstack_restore(kernel_stack_top),
                    memory_set: MemorySet::from_existed_user(&parent_inner.memory_set),
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    base_size: parent_inner.base_size,
                    exit_code: 0,
                })
            },
        });

        // 同时更新父任务状态
        parent_inner.children.push(task_ctrl_block.clone());

        // 异常上下文中的 `kernel_sp` 字段需修改
        let trap_cx = task_ctrl_block.inner_exclusive_access().get_trap_cx();
        trap_cx.kernel_sp = kernel_stack_top;

        task_ctrl_block
    }

    // 为任务载入可执行程序
    pub fn exec(&self, elf_data: &[u8]) {
        // 主要修改任务的地址空间和异常上下文
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf_data(elf_data);
        let mut inner = self.inner_exclusive_access();
        inner.memory_set = memory_set;
        let trap_cx = inner.get_trap_cx();
        *trap_cx = TrapContext::init_app_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            self.kernel_stack.get_top(),
            trap_handler as *const() as usize
        );
    }

    pub fn inner_exclusive_access(&self) -> RefMut<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }

    pub fn getpid(&self) -> usize {
        self.pid.0
    }
}

/// 任务控制块内部数据
/// 
/// - 功能：用于保存任务的执行状态、地址空间、父子任务关系等信息
/// - 内容:
///     - `task_status`   任务状态
///     - `task_context`  任务上下文
///     - `memory_set`    任务的地址空间
///     - ~~`trap_cx_ppn` 任务的异常上下文所在物理页号~~ 原字段只是为了获取页帧上的数据，现在使用更~~安全~~高效的方法获取
///     - `parent`        任务的父任务（进程），使用弱引用避免环形引用
///     - `children`      任务的子任务（进程），使用原子计数引用
///     - `base_size`     任务的内存空间大小 -> 暂时不知道有啥用，这里直接取栈顶的数值
///     - `exit_code`     任务退出码
pub struct TaskControlBlockInner {
    pub task_status: TaskStatus,
    pub task_context: TaskContext,
    pub memory_set: MemorySet,
    pub parent: Option<Weak<TaskControlBlock>>,
    pub children: Vec<Arc<TaskControlBlock>>,
    pub base_size: usize,
    pub exit_code: i32,
}

impl TaskControlBlockInner {
    /// 获取内核栈上的用户程序异常上下文
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        let trap_cx_ppn: PhysAddr = self
            .memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT).into())
            .unwrap()
            .ppn()
            .into();
        unsafe {
            &mut *(trap_cx_ppn.0 as *mut TrapContext)
        }
    }
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