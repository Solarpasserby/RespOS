use crate::mutex::SpinLock;
use crate::task::wakeup_task;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub type PollEvents = u16;

pub const POLL_READ: PollEvents = 1 << 0;
pub const POLL_WRITE: PollEvents = 1 << 1;
pub const POLL_HUP: PollEvents = 1 << 2;

pub struct PollWaiters {
    waiters: SpinLock<BTreeMap<usize, PollEvents>>,
}

impl PollWaiters {
    pub fn new() -> Self {
        Self {
            waiters: SpinLock::new(BTreeMap::new()),
        }
    }

    pub fn register(&self, tid: usize, events: PollEvents) {
        if events == 0 {
            return;
        }
        self.waiters
            .lock()
            .entry(tid)
            .and_modify(|registered| *registered |= events)
            .or_insert(events);
    }

    pub fn unregister(&self, tid: usize) {
        self.waiters.lock().remove(&tid);
    }

    pub fn notify(&self, events: PollEvents) {
        let tids = {
            let mut waiters = self.waiters.lock();
            let tids: Vec<usize> = waiters
                .iter()
                .filter_map(|(&tid, &registered)| (registered & events != 0).then_some(tid))
                .collect();
            for tid in &tids {
                waiters.remove(tid);
            }
            tids
        };
        for tid in tids {
            wakeup_task(tid);
        }
    }
}
