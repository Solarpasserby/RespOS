use super::{Errno, SysResult};
use crate::config::PAGE_SIZE;
use crate::mm::{FrameTracker, MapPermission, VirtAddr, frame_alloc};
use crate::task::current_task;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

const IPC_PRIVATE: isize = 0;
const IPC_CREAT: usize = 0o1000;
const IPC_RMID: usize = 0;
const SHM_RDONLY: usize = 0o10000;

struct ShmSegment {
    size: usize,
    frames: Vec<Arc<FrameTracker>>,
}

struct ShmTable {
    next_id: usize,
    segments: BTreeMap<usize, ShmSegment>,
}

impl ShmTable {
    fn new() -> Self {
        Self {
            next_id: 1,
            segments: BTreeMap::new(),
        }
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

lazy_static! {
    static ref SHM_TABLE: Mutex<ShmTable> = Mutex::new(ShmTable::new());
}

pub fn sys_shmget(key: isize, size: usize, shmflg: usize) -> SysResult<usize> {
    if size == 0 {
        return Err(Errno::EINVAL);
    }
    if key != IPC_PRIVATE && (shmflg & IPC_CREAT) == 0 {
        return Err(Errno::ENOENT);
    }

    let map_len = size.checked_add(PAGE_SIZE - 1).ok_or(Errno::ENOMEM)? & !(PAGE_SIZE - 1);
    let page_count = map_len / PAGE_SIZE;
    let mut frames = Vec::new();
    for _ in 0..page_count {
        frames.push(Arc::new(frame_alloc().ok_or(Errno::ENOMEM)?));
    }

    let mut table = SHM_TABLE.lock();
    let id = table.alloc_id();
    table.segments.insert(
        id,
        ShmSegment {
            size: map_len,
            frames,
        },
    );
    Ok(id)
}

pub fn sys_shmat(shmid: usize, shmaddr: usize, shmflg: usize) -> SysResult<usize> {
    let (size, frames) = {
        let table = SHM_TABLE.lock();
        let segment = table.segments.get(&shmid).ok_or(Errno::EINVAL)?;
        (segment.size, segment.frames.clone())
    };

    let addr = if shmaddr == 0 {
        None
    } else if shmaddr % PAGE_SIZE == 0 {
        Some(shmaddr)
    } else {
        return Err(Errno::EINVAL);
    };

    let mut permission = MapPermission::READ | MapPermission::USER;
    if (shmflg & SHM_RDONLY) == 0 {
        permission |= MapPermission::WRITE;
    }

    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_write(|memory_set| {
        let start = memory_set.mmap_shared_frames(addr, size, permission, &frames)?;
        memory_set.flush_tlb();
        Ok(start)
    })
}

pub fn sys_shmctl(shmid: usize, cmd: usize, _buf: usize) -> SysResult<usize> {
    match cmd {
        IPC_RMID => {
            let mut table = SHM_TABLE.lock();
            table.segments.remove(&shmid).ok_or(Errno::EINVAL)?;
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
}

pub fn sys_shmdt(shmaddr: usize) -> SysResult<usize> {
    if shmaddr % PAGE_SIZE != 0 {
        return Err(Errno::EINVAL);
    }
    let task = current_task().expect("[kernel] current task is None.");
    task.op_memory_set_write(|memory_set| {
        memory_set.remove_area_with_start_vpn(VirtAddr::from(shmaddr).floor())?;
        memory_set.flush_tlb();
        Ok(0)
    })
}
