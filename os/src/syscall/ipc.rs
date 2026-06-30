use super::{Errno, SysResult};
use crate::config::PAGE_SIZE;
use crate::mm::{FrameTracker, MapPermission, MmapBacking, VirtAddr, copy_from_user, copy_to_user};
use crate::task::{TASK_MANAGER, current_task};
use crate::timer::get_time_ms;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;

const IPC_PRIVATE: isize = 0;
const IPC_CREAT: usize = 0o1000;
const IPC_EXCL: usize = 0o2000;
const IPC_RMID: usize = 0;
const IPC_SET: usize = 1;
const IPC_STAT: usize = 2;
const IPC_INFO: usize = 3;

const SHM_RDONLY: usize = 0o10000;
const SHM_RND: usize = 0o20000;
const SHM_REMAP: usize = 0o40000;
const SHM_HUGETLB: usize = 0o4000;
const SHM_LOCK: usize = 11;
const SHM_UNLOCK: usize = 12;
const SHM_STAT: usize = 13;
const SHM_INFO: usize = 14;
const SHM_STAT_ANY: usize = 15;
const SHM_DEST: u32 = 0o1000;
const SHM_LOCKED: u32 = 0o2000;

const SHMMNI: usize = 4096;
const SHMMIN: usize = 1;
const DEFAULT_SHMMAX: usize = usize::MAX - (1 << 24);
const DEFAULT_SHMALL: usize = usize::MAX / PAGE_SIZE;
const MODE_MASK: u32 = 0o777;
#[cfg(target_arch = "loongarch64")]
const SHMLBA: usize = 0x10000;
#[cfg(not(target_arch = "loongarch64"))]
const SHMLBA: usize = PAGE_SIZE;

static SHMMAX_VALUE: AtomicUsize = AtomicUsize::new(DEFAULT_SHMMAX);
static SHMMNI_VALUE: AtomicUsize = AtomicUsize::new(SHMMNI);
static SHMALL_VALUE: AtomicUsize = AtomicUsize::new(DEFAULT_SHMALL);

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct IpcPerm {
    key: i32,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    mode: u32,
    seq: i32,
    pad1: isize,
    pad2: isize,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ShmidDs {
    shm_perm: IpcPerm,
    shm_segsz: usize,
    shm_atime: isize,
    shm_dtime: isize,
    shm_ctime: isize,
    shm_cpid: i32,
    shm_lpid: i32,
    shm_nattch: usize,
    pad1: usize,
    pad2: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ShmInfoLimits {
    shmmax: usize,
    shmmin: usize,
    shmmni: usize,
    shmseg: usize,
    shmall: usize,
    unused: [usize; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ShmInfo {
    used_ids: i32,
    shm_tot: usize,
    shm_rss: usize,
    shm_swp: usize,
    swap_attempts: usize,
    swap_successes: usize,
}

struct ShmSegment {
    key: isize,
    id: usize,
    index: usize,
    size: usize,
    map_len: usize,
    mode: u32,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    cpid: i32,
    lpid: i32,
    atime: isize,
    dtime: isize,
    ctime: isize,
    marked_removed: bool,
    locked: bool,
    frames: Vec<Arc<FrameTracker>>,
}

struct ShmTable {
    next_id: usize,
    next_index: usize,
    next_attach_id: usize,
    segments: BTreeMap<usize, ShmSegment>,
    attach_owners: BTreeMap<usize, usize>,
}

impl ShmTable {
    fn new() -> Self {
        Self {
            next_id: 1,
            next_index: 0,
            next_attach_id: 1,
            segments: BTreeMap::new(),
            attach_owners: BTreeMap::new(),
        }
    }

    fn alloc_id(&mut self) -> SysResult<(usize, usize)> {
        if self.segments.len() >= shmmni_value() {
            return Err(Errno::ENOSPC);
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(Errno::ENOSPC)?;
        let index = self.next_index;
        self.next_index = self.next_index.checked_add(1).ok_or(Errno::ENOSPC)?;
        Ok((id, index))
    }

    fn find_by_key(&self, key: isize) -> Option<&ShmSegment> {
        self.segments
            .values()
            .find(|segment| segment.key == key && !segment.marked_removed)
    }

    fn find_id_by_index(&self, index: usize) -> Option<usize> {
        self.segments
            .iter()
            .find_map(|(id, segment)| (segment.index == index).then_some(*id))
    }

    fn max_index(&self) -> usize {
        self.segments
            .values()
            .map(|segment| segment.index)
            .max()
            .unwrap_or(0)
    }

    fn alloc_attach_id(&mut self) -> SysResult<usize> {
        let id = self.next_attach_id;
        self.next_attach_id = self.next_attach_id.checked_add(1).ok_or(Errno::ENOSPC)?;
        Ok(id)
    }

    fn remove_segment(&mut self, shmid: usize) {
        self.segments.remove(&shmid);
        self.attach_owners.retain(|_, owner| *owner != shmid);
    }
}

lazy_static! {
    static ref SHM_TABLE: Mutex<ShmTable> = Mutex::new(ShmTable::new());
}

pub(crate) fn shmmax_value() -> usize {
    SHMMAX_VALUE.load(Ordering::Relaxed)
}

pub(crate) fn shmmni_value() -> usize {
    SHMMNI_VALUE.load(Ordering::Relaxed)
}

pub(crate) fn shmall_value() -> usize {
    SHMALL_VALUE.load(Ordering::Relaxed)
}

pub(crate) fn set_shmmax_value(value: usize) -> SysResult<()> {
    SHMMAX_VALUE.store(value, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn set_shmmni_value(value: usize) -> SysResult<()> {
    SHMMNI_VALUE.store(value, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn set_shmall_value(value: usize) -> SysResult<()> {
    SHMALL_VALUE.store(value, Ordering::Relaxed);
    Ok(())
}

fn now_sec() -> isize {
    (get_time_ms() / 1000) as isize
}

fn current_ids() -> (u32, u32, i32) {
    let task = current_task().expect("[kernel] current task is None.");
    (task.euid() as u32, task.egid() as u32, task.tgid() as i32)
}

fn shm_access_allowed(segment: &ShmSegment, requested: usize) -> bool {
    if requested == 0 {
        return true;
    }
    let task = current_task().expect("[kernel] current task is None.");
    if task.euid() == 0 {
        return true;
    }

    let shift = if task.euid() as u32 == segment.uid || task.euid() as u32 == segment.cuid {
        6
    } else if task.egid() as u32 == segment.gid || task.egid() as u32 == segment.cgid {
        3
    } else {
        0
    };
    let available = (segment.mode >> shift) & 0o7;
    let mut needed = 0u32;
    if requested & 0o400 != 0 {
        needed |= 0o4;
    }
    if requested & 0o200 != 0 {
        needed |= 0o2;
    }
    available & needed == needed
}

fn shm_attach_count(frames: &[Arc<FrameTracker>]) -> usize {
    let mut count = 0usize;
    TASK_MANAGER.for_each(|task| {
        count += task
            .op_memory_set_read(|memory_set| memory_set.shm_attach_ids_for_frames(frames).len());
    });
    count
}

fn shmid_ds(segment: &ShmSegment, nattch: usize) -> ShmidDs {
    let mut mode = segment.mode;
    if segment.marked_removed {
        mode |= SHM_DEST;
    }
    if segment.locked {
        mode |= SHM_LOCKED;
    }
    ShmidDs {
        shm_perm: IpcPerm {
            key: segment.key as i32,
            uid: segment.uid,
            gid: segment.gid,
            cuid: segment.cuid,
            cgid: segment.cgid,
            mode,
            seq: 0,
            pad1: 0,
            pad2: 0,
        },
        shm_segsz: segment.size,
        shm_atime: segment.atime,
        shm_dtime: segment.dtime,
        shm_ctime: segment.ctime,
        shm_cpid: segment.cpid,
        shm_lpid: segment.lpid,
        shm_nattch: nattch,
        pad1: 0,
        pad2: 0,
    }
}

fn lookup_shm_id(table: &ShmTable, shmid: usize, by_index: bool) -> SysResult<usize> {
    if by_index {
        table.find_id_by_index(shmid).ok_or(Errno::EINVAL)
    } else if table.segments.contains_key(&shmid) {
        Ok(shmid)
    } else {
        Err(Errno::EINVAL)
    }
}

pub fn sys_shmget(key: isize, size: usize, shmflg: usize) -> SysResult<usize> {
    let mut table = SHM_TABLE.lock();
    if shmflg & SHM_HUGETLB != 0 {
        return Err(Errno::EINVAL);
    }
    if key != IPC_PRIVATE {
        if let Some(segment) = table.find_by_key(key) {
            if shmflg & IPC_CREAT != 0 && shmflg & IPC_EXCL != 0 {
                return Err(Errno::EEXIST);
            }
            if size > segment.size {
                return Err(Errno::EINVAL);
            }
            if !shm_access_allowed(segment, shmflg & 0o600) {
                return Err(Errno::EACCES);
            }
            return Ok(segment.id);
        }
        if shmflg & IPC_CREAT == 0 {
            return Err(Errno::ENOENT);
        }
    }

    if size < SHMMIN || size > shmmax_value() {
        return Err(Errno::EINVAL);
    }
    let map_len = size.checked_add(PAGE_SIZE - 1).ok_or(Errno::ENOMEM)? & !(PAGE_SIZE - 1);
    let page_count = map_len / PAGE_SIZE;
    let used_pages: usize = table
        .segments
        .values()
        .map(|segment| segment.map_len / PAGE_SIZE)
        .sum();
    if used_pages.checked_add(page_count).ok_or(Errno::ENOSPC)? > shmall_value() {
        return Err(Errno::ENOSPC);
    }
    let mut frames = Vec::new();
    for _ in 0..page_count {
        frames.push(Arc::new(crate::mm::frame_alloc().ok_or(Errno::ENOMEM)?));
    }

    let (uid, gid, pid) = current_ids();
    let (id, index) = table.alloc_id()?;
    table.segments.insert(
        id,
        ShmSegment {
            key,
            id,
            index,
            size,
            map_len,
            mode: (shmflg as u32) & MODE_MASK,
            uid,
            gid,
            cuid: uid,
            cgid: gid,
            cpid: pid,
            lpid: 0,
            atime: 0,
            dtime: 0,
            ctime: now_sec(),
            marked_removed: false,
            locked: false,
            frames,
        },
    );
    Ok(id)
}

pub fn sys_shmat(shmid: usize, shmaddr: usize, shmflg: usize) -> SysResult<usize> {
    let (map_len, frames, readonly, attach_id) = {
        let mut table = SHM_TABLE.lock();
        let segment = table.segments.get_mut(&shmid).ok_or(Errno::EINVAL)?;
        let readonly = (shmflg & SHM_RDONLY) != 0;
        let needed = if readonly { 0o400 } else { 0o600 };
        if !shm_access_allowed(segment, needed) {
            return Err(Errno::EACCES);
        }
        segment.atime = now_sec();
        segment.lpid = current_task()
            .expect("[kernel] current task is None.")
            .tgid() as i32;
        let map_len = segment.map_len;
        let frames = segment.frames.clone();
        let attach_id = table.alloc_attach_id()?;
        table.attach_owners.insert(attach_id, shmid);
        (map_len, frames, readonly, attach_id)
    };

    let addr = if shmaddr == 0 {
        if shmflg & SHM_REMAP != 0 {
            return Err(Errno::EINVAL);
        }
        None
    } else if shmaddr % PAGE_SIZE == 0 {
        Some(shmaddr)
    } else if shmflg & SHM_RND != 0 {
        let rounded = shmaddr & !(SHMLBA - 1);
        if rounded == 0 && shmflg & SHM_REMAP != 0 {
            return Err(Errno::EINVAL);
        }
        Some(rounded)
    } else {
        return Err(Errno::EINVAL);
    };

    let mut permission = MapPermission::READ | MapPermission::USER;
    if !readonly {
        permission |= MapPermission::WRITE;
    }

    let task = current_task().expect("[kernel] current task is None.");
    let result = task.op_memory_set_write(|memory_set| {
        let start = memory_set.mmap_area(
            addr,
            map_len,
            permission,
            shmflg & SHM_REMAP != 0,
            false,
            false,
            MmapBacking::SharedFrames {
                attach_id,
                frames: frames.as_slice(),
            },
        )?;
        memory_set.flush_tlb();
        Ok(start)
    });
    if result.is_err() {
        SHM_TABLE.lock().attach_owners.remove(&attach_id);
    }
    result
}

pub fn sys_shmctl(shmid: usize, cmd: usize, buf: usize) -> SysResult<usize> {
    match cmd {
        IPC_INFO => {
            let info = ShmInfoLimits {
                shmmax: shmmax_value(),
                shmmin: SHMMIN,
                shmmni: shmmni_value(),
                shmseg: shmmni_value(),
                shmall: shmall_value(),
                unused: [0; 4],
            };
            copy_to_user(buf as *mut ShmInfoLimits, &info as *const ShmInfoLimits, 1)?;
            Ok(SHM_TABLE.lock().max_index())
        }
        SHM_INFO => {
            let table = SHM_TABLE.lock();
            let pages = table
                .segments
                .values()
                .map(|segment| segment.map_len / PAGE_SIZE)
                .sum();
            let info = ShmInfo {
                used_ids: table.segments.len() as i32,
                shm_tot: pages,
                shm_rss: pages,
                shm_swp: 0,
                swap_attempts: 0,
                swap_successes: 0,
            };
            copy_to_user(buf as *mut ShmInfo, &info as *const ShmInfo, 1)?;
            Ok(table.max_index())
        }
        IPC_STAT | SHM_STAT | SHM_STAT_ANY => {
            let by_index = cmd == SHM_STAT || cmd == SHM_STAT_ANY;
            let (id, frames, ds) = {
                let table = SHM_TABLE.lock();
                let id = lookup_shm_id(&table, shmid, by_index)?;
                let segment = table.segments.get(&id).ok_or(Errno::EINVAL)?;
                if cmd != SHM_STAT_ANY && !shm_access_allowed(segment, 0o400) {
                    return Err(Errno::EACCES);
                }
                let frames = segment.frames.clone();
                (id, frames, shmid_ds(segment, 0))
            };
            let mut ds = ds;
            ds.shm_nattch = shm_attach_count(&frames);
            copy_to_user(buf as *mut ShmidDs, &ds as *const ShmidDs, 1)?;
            if by_index { Ok(id) } else { Ok(0) }
        }
        IPC_SET => {
            let mut user_ds = ShmidDs::default();
            copy_from_user(&mut user_ds as *mut ShmidDs, buf as *const ShmidDs, 1)?;
            let mut table = SHM_TABLE.lock();
            let segment = table.segments.get_mut(&shmid).ok_or(Errno::EINVAL)?;
            let task = current_task().expect("[kernel] current task is None.");
            if task.euid() != 0 && task.euid() as u32 != segment.uid {
                return Err(Errno::EPERM);
            }
            segment.uid = user_ds.shm_perm.uid;
            segment.gid = user_ds.shm_perm.gid;
            segment.mode = user_ds.shm_perm.mode & MODE_MASK;
            segment.ctime = now_sec();
            Ok(0)
        }
        IPC_RMID => {
            let mut table = SHM_TABLE.lock();
            let segment = table.segments.get_mut(&shmid).ok_or(Errno::EINVAL)?;
            let task = current_task().expect("[kernel] current task is None.");
            if task.euid() != 0 && task.euid() as u32 != segment.uid {
                return Err(Errno::EPERM);
            }
            let frames = segment.frames.clone();
            if shm_attach_count(&frames) == 0 {
                table.remove_segment(shmid);
            } else {
                segment.marked_removed = true;
                segment.ctime = now_sec();
            }
            Ok(0)
        }
        SHM_LOCK | SHM_UNLOCK => {
            let mut table = SHM_TABLE.lock();
            let segment = table.segments.get_mut(&shmid).ok_or(Errno::EINVAL)?;
            let task = current_task().expect("[kernel] current task is None.");
            if task.euid() != 0 && task.euid() as u32 != segment.uid {
                return Err(Errno::EPERM);
            }
            segment.locked = cmd == SHM_LOCK;
            segment.ctime = now_sec();
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
    let attach_id = task.op_memory_set_write(|memory_set| {
        let attach_id = memory_set.remove_shm_attachment(VirtAddr::from(shmaddr).floor())?;
        memory_set.flush_tlb();
        Ok(attach_id)
    })?;

    let mut table = SHM_TABLE.lock();
    let pid = task.tgid() as i32;
    let now = now_sec();
    if let Some(shmid) = table.attach_owners.remove(&attach_id) {
        if let Some(segment) = table.segments.get_mut(&shmid) {
            segment.lpid = pid;
            segment.dtime = now;
        }
    }
    let remove_ids: Vec<usize> = table
        .segments
        .iter()
        .filter_map(|(id, segment)| {
            (segment.marked_removed && shm_attach_count(&segment.frames) == 0).then_some(*id)
        })
        .collect();
    for id in remove_ids {
        table.remove_segment(id);
    }
    Ok(0)
}
