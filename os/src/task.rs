// os/src/task.rs

//! ### ~~线程~~任务模块
//! 
//! 主要实现任务调度，实现 CPU 时间资源分配

mod context;
mod switch;

use alloc::vec::Vec;
use lazy_static::lazy_static;
use crate::config::{ KERNEL_STACK_SIZE, TRAP_CONTEXT, get_kernel_stack_top };
use crate::sync::UPSafeCell;
use crate::loader::{ get_app_data, get_app_num };
use crate::trap::{ TrapContext, trap_handler };
use crate::mm::{ KERNEL_SPACE, MemorySet, VirtAddr, PhysAddr };
use crate::sbi::shutdown;
use switch::__switch;
use context::TaskContext;


lazy_static! {
    pub static ref TASK_MANAGER: TaskManager = {
        println!("[kernel] init TASK_MANAGER.");
        let app_num = get_app_num();
        let mut tasks: Vec<TaskControlBlock> = Vec::new();
        for i in 0..app_num {
            tasks.push(TaskControlBlock::new(
                get_app_data(i),
                i,
            ));
        }
        TaskManager {
            app_num,
            inner: unsafe {
                UPSafeCell::new( TaskManagerInner {
                    tasks,
                    current_task: 0,
                })
            }
        }
    };
}

/// 任务调度管理器
/// 
/// TODO: 当前功能相当孱弱，之后需要使用更优秀的调度算法
pub struct TaskManager {
    app_num: usize,
    inner: UPSafeCell<TaskManagerInner>,
}

struct TaskManagerInner {
    tasks: Vec<TaskControlBlock>,
    current_task: usize,
}

impl TaskManager {
    fn mark_current_suspended(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Ready;
    }

    fn mark_current_exited(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Exited;
    }

    fn run_next_task(&self) { // 我不太确定我需不需要把返回值改成 !
        if let Some(next) = self.find_next_task() {
            let mut inner = self.inner.exclusive_access();
            let current = inner.current_task;
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task = next;
            let current_task_context_ptr = &mut inner.tasks[current].task_context as *mut TaskContext;
            let next_task_context_ptr = &inner.tasks[next].task_context as *const TaskContext;
            drop(inner); // __switch 调用后不会返回，因此需要主动释放变量

            unsafe {
                __switch(current_task_context_ptr, next_task_context_ptr);
            }
        } else {
            // 所有用户程序执行完成后关闭系统
            println!("All applications completed!");
            shutdown(false);
        }
    }

    fn find_next_task(&self) -> Option<usize> {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        (current + 1..current + self.app_num + 1) // 从 `current` 开始循环遍历
            .map(|i| i % self.app_num)
            .find(|i| {
                inner.tasks[*i].task_status == TaskStatus::Ready 
            })
    }

    fn start_running_tasks(&self) -> ! {
        let mut inner = self.inner.exclusive_access();
        let first_task = &mut inner.tasks[0];
        first_task.task_status = TaskStatus::Running;
        let next_task_context_ptr = &first_task.task_context as *const TaskContext;
        let current_task_context_ptr = &mut TaskContext::init_zero() as *mut TaskContext;
        drop(inner); // 该函数不会返回，因此需要主动释放变量

        unsafe {
            __switch(current_task_context_ptr, next_task_context_ptr);
        };
        
        panic!("Unreachable while start running tasks");
    }

    fn get_current_token(&self) -> usize {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].get_user_token()
    }

    fn with_current_trap_cx(&self, f: impl FnOnce(&mut TrapContext)) {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        f(inner.tasks[current].get_trap_cx())
    }
}


/// 任务控制块
/// 
/// - 功能：用于保存任务的执行状态和相关信息
/// - 内容:
///     - `task_status`   任务状态
///     - `task_context`  任务上下文
///     - `memory_set`    任务的地址空间
///     - ~~`trap_cx_ppn` 任务的异常上下文所在物理页号~~ 原字段只是为了获取页帧上的数据，现在使用更安全的方法获取
///     - `base_size`     任务的内存空间大小 -> 暂时不知道有啥用，这里直接取栈顶的数值
struct TaskControlBlock {
    pub task_status: TaskStatus,
    pub task_context: TaskContext,
    pub memory_set: MemorySet,
    // pub trap_cx_ppn: PhysPageNum,
    #[allow(unused)]
    pub base_size: usize,
}

impl TaskControlBlock {
    pub fn new(elf_data: &[u8], app_id: usize) -> Self {
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf_data(elf_data);
        let kernel_stack_top = get_kernel_stack_top(app_id);
        // 在内核空间额外为用户程序分配并映射一个内核栈
        KERNEL_SPACE
            .exclusive_access()
            .insert_stack_area(
                VirtAddr::from(kernel_stack_top - KERNEL_STACK_SIZE),
                VirtAddr::from(kernel_stack_top),
            );
        let task_ctrl_block = Self {
            task_status: TaskStatus::Ready,
            task_context: TaskContext::create_for_kstack_restore(kernel_stack_top),
            memory_set,
            base_size: user_sp,
        };
        let trap_cx = task_ctrl_block.get_trap_cx();
        *trap_cx = TrapContext::init_app_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as *const() as usize
        );

        task_ctrl_block
    }

    /// 获取用户任务页表的 `stap` 寄存器值
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }

    /// 获取内核栈上用户程序异常上下文中的头部数据
    fn get_trap_cx(&self) -> &mut TrapContext {
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
}

/// 任务状态
#[derive(Copy, Clone, PartialEq)]
enum TaskStatus {
    Ready,   // 已就绪
    Running, // 正在运行
    Exited,  // 已退出
}


fn mark_current_suspended() {
    TASK_MANAGER.mark_current_suspended();
}

fn mark_current_exited() {
    TASK_MANAGER.mark_current_exited();
}

fn run_next_task() {
    TASK_MANAGER.run_next_task();
}

pub fn start_running_tasks() -> ! {
    TASK_MANAGER.start_running_tasks()
}

pub fn suspend_current_and_run_next() {
    mark_current_suspended();
    run_next_task();
}

pub fn exit_current_and_run_next() {
    mark_current_exited();
    run_next_task();
}

pub fn current_user_token() -> usize {
    TASK_MANAGER.get_current_token()
}

pub fn with_current_trap_cx(f: impl FnOnce(&mut TrapContext)) {
    TASK_MANAGER.with_current_trap_cx(f)
}
