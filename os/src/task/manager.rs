// os/src/task/manager.rs

use super::TaskControlBlock;
use crate::mutex::SpinNoIrqLock;
use alloc::sync::{Arc, Weak};
use hashbrown::HashMap;
use lazy_static::lazy_static;

// 任务管理器
lazy_static! {
    pub static ref TASK_MANAGER: TaskManager = TaskManager::new();
}

pub struct TaskManager(SpinNoIrqLock<HashMap<usize, Weak<TaskControlBlock>>>);

impl TaskManager {
    pub fn new() -> Self {
        Self(SpinNoIrqLock::new(HashMap::new()))
    }

    pub fn add(&self, task: &Arc<TaskControlBlock>) {
        self.0.lock().insert(task.tid(), Arc::downgrade(task));
    }

    pub fn remove(&self, tid: usize) {
        self.0.lock().remove(&tid);
    }

    pub fn len(&self) -> usize {
        self.0.lock().len()
    }

    pub fn get(&self, tid: usize) -> Option<Arc<TaskControlBlock>> {
        match self.0.lock().get(&tid) {
            Some(task) => task.upgrade(),
            None => None,
        }
    }

    pub fn for_each(&self, mut f: impl FnMut(&Arc<TaskControlBlock>)) {
        for task in self.0.lock().values() {
            f(&task.upgrade().unwrap())
        }
    }
}
