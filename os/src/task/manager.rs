// os/src/task/manager.rs

use super::task::TaskControlBlock;
use alloc::collections::BTreeMap;
use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use lazy_static::lazy_static;
use spin::Mutex;


lazy_static! {
    /// 任务管理器
    pub static ref TASK_MANAGER: Mutex<TaskManager> = Mutex::new(TaskManager::new());
    pub static ref PID2TCB: Mutex<BTreeMap<usize, Arc<TaskControlBlock>>> = Mutex::new(BTreeMap::new());
}

/// 任务调度管理器
///
/// 关注于实现任务的调度，具体算法依赖实现，这里使用最简单的 RR 算法
///
/// 注：任务管理应做到任务调度和运行状态切换两个功能，前者在此实现，后者由 [`Processor`] 实现
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }

    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.ready_queue.pop_front()
    }
}

/// 添加任务
pub fn add_task(task: Arc<TaskControlBlock>) {
    TASK_MANAGER.lock().add(task);
}
/// 获取任务
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    TASK_MANAGER.lock().fetch()
}
/// 根据进程id查找进程
pub fn pid2task(pid: usize) -> Option<Arc<TaskControlBlock>> {
    // 关键：用 &*PID2TCB 来拿到内部的 Mutex 引用
    let map_guard = { &*PID2TCB }.lock();
    map_guard.get(&pid).map(Arc::clone)
}

