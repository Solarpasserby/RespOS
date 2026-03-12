//! ~~线程~~任务模块
//! 
//! 主要实现任务调度，实现 CPU 时间资源分配

mod context;
mod switch;

use lazy_static::lazy_static;
use crate::config::MAX_APP_NUM;
use crate::sync::UPSafeCell;
use crate::loader::{get_app_num, init_app_context};
use crate::task::switch::__switch;
use context::TaskContext;

/// 任务状态
#[derive(Copy, Clone, PartialEq)]
enum TaskStatus {
    Uninit,  // 未初始化
    Ready,   // 已就绪
    Running, // 正在运行
    Exited,  // 已退出
}

/// 任务控制块
#[derive(Copy, Clone)]
struct TaskControlBlock {
    pub task_status: TaskStatus,
    pub task_context: TaskContext,
}

pub struct TaskManager {
    app_num: usize,
    inner: UPSafeCell<TaskManagerInner>,
}

struct TaskManagerInner {
    tasks: [TaskControlBlock; MAX_APP_NUM],
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
            panic!("All applications completed!");
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
}

lazy_static! {
    pub static ref TASK_MANAGER: TaskManager = {
        let app_num = get_app_num();
        let mut tasks = [
            TaskControlBlock {
                task_status: TaskStatus::Uninit,
                task_context: TaskContext::init_zero(),
            };
            MAX_APP_NUM
        ];
        for i in 0..app_num {
            tasks[i].task_status = TaskStatus::Ready;
            tasks[i].task_context = TaskContext::create_for_kstack_restore(init_app_context(i));
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
