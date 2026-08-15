#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    exit, fork, getgid, getpid, getuid, mmap_raw, munmap, setgid, setuid, shmat, shmctl, shmdt,
    shmget, waitpid,
};

const PAGE_SIZE: usize = 4096;
const SEGMENT_SIZE: usize = PAGE_SIZE + 17;
const IPC_PRIVATE: isize = 0;
const IPC_CREAT: usize = 0o1000;
const IPC_EXCL: usize = 0o2000;
const IPC_RMID: usize = 0;
const IPC_SET: usize = 1;
const IPC_STAT: usize = 2;
const IPC_INFO: usize = 3;
const SHM_STAT: usize = 13;
const SHM_INFO: usize = 14;
const SHM_STAT_ANY: usize = 15;
const SHM_DEST: u32 = 0o1000;
const SHM_RDONLY: usize = 0o10000;
const EINVAL: isize = 22;
const EACCES: isize = 13;
const EEXIST: isize = 17;
const EPERM: isize = 1;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;

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

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct PermissionResults {
    setgid: isize,
    setuid: isize,
    uid: isize,
    gid: isize,
    lookup_none: isize,
    lookup_read: isize,
    shmat_read: isize,
    unexpected_detach: isize,
    ipc_stat: isize,
    shm_stat: isize,
    shm_stat_any: isize,
    ipc_set: isize,
    ipc_rmid: isize,
}

fn read_stat(shmid: usize) -> ShmidDs {
    let mut ds = ShmidDs::default();
    let ds_ptr = &mut ds as *mut ShmidDs as usize;
    assert_eq!(shmctl(shmid, IPC_STAT, ds_ptr), 0);
    ds
}

fn read_info() -> ShmInfo {
    let mut info = ShmInfo::default();
    let info_ptr = &mut info as *mut ShmInfo as usize;
    assert!(shmctl(0, SHM_INFO, info_ptr) >= 0);
    info
}

fn find_index(shmid: usize) -> usize {
    let mut limits = ShmInfoLimits::default();
    let limits_ptr = &mut limits as *mut ShmInfoLimits as usize;
    let max_index = shmctl(0, IPC_INFO, limits_ptr);
    assert!(max_index >= 0);
    for index in 0..=max_index as usize {
        let mut ds = ShmidDs::default();
        let ds_ptr = &mut ds as *mut ShmidDs as usize;
        let result = shmctl(index, SHM_STAT, ds_ptr);
        if result == shmid as isize {
            return index;
        }
        assert!(result >= 0 || result == -EINVAL || result == -EACCES);
    }
    panic!("created shmid was not reachable through SHM_STAT");
}

fn create_keyed_segment(mode: usize) -> (isize, usize) {
    let mut key = 0x5300_0000isize ^ getpid();
    for _ in 0..256 {
        let shmid = shmget(key, PAGE_SIZE, IPC_CREAT | IPC_EXCL | mode);
        if shmid > 0 {
            return (key, shmid as usize);
        }
        assert_eq!(shmid, -EEXIST);
        key += 1;
    }
    panic!("could not allocate a unique SysV SHM key");
}

fn verify_permission_matrix() -> bool {
    let (key, shmid) = create_keyed_segment(0);
    let index = find_index(shmid);
    let control = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    assert!(control > 0, "permission control mmap failed: {}", control);
    let control = control as usize;

    let child = fork();
    assert!(child >= 0, "permission fork failed: {}", child);
    if child == 0 {
        let results = unsafe { &mut *(control as *mut PermissionResults) };
        results.setgid = setgid(65534);
        results.setuid = setuid(65534);
        results.uid = getuid();
        results.gid = getgid();
        results.lookup_none = shmget(key, 0, 0);
        results.lookup_read = shmget(key, 0, 0o400);
        results.shmat_read = shmat(shmid, 0, SHM_RDONLY);
        results.unexpected_detach = if results.shmat_read > 0 {
            shmdt(results.shmat_read as usize)
        } else {
            0
        };

        let mut ds = ShmidDs::default();
        let ds_ptr = &mut ds as *mut ShmidDs as usize;
        results.ipc_stat = shmctl(shmid, IPC_STAT, ds_ptr);
        results.shm_stat = shmctl(index, SHM_STAT, ds_ptr);
        results.shm_stat_any = shmctl(index, SHM_STAT_ANY, ds_ptr);
        results.ipc_set = shmctl(shmid, IPC_SET, ds_ptr);
        results.ipc_rmid = shmctl(shmid, IPC_RMID, 0);
        let _ = exit(0);
        return true;
    }

    let mut status = 0;
    let waited = waitpid(child as usize, &mut status);
    let results = unsafe { *(control as *const PermissionResults) };
    let cleanup = shmctl(shmid, IPC_RMID, 0);
    let unmap = munmap(control, PAGE_SIZE);

    assert_eq!(waited, child);
    assert_eq!(status, 0);
    assert_eq!(results.setgid, 0);
    assert_eq!(results.setuid, 0);
    assert_eq!(results.uid, 65534);
    assert_eq!(results.gid, 65534);
    assert_eq!(results.lookup_none, shmid as isize);
    assert_eq!(results.lookup_read, -EACCES);
    assert_eq!(results.shmat_read, -EACCES);
    assert_eq!(results.unexpected_detach, 0);
    assert_eq!(results.ipc_stat, -EACCES);
    assert_eq!(results.shm_stat, -EACCES);
    assert_eq!(results.shm_stat_any, shmid as isize);
    assert_eq!(results.ipc_set, -EPERM);
    assert_eq!(results.ipc_rmid, -EPERM);
    assert_eq!(cleanup, 0);
    assert_eq!(unmap, 0);
    false
}

fn read_byte(addr: usize) -> u8 {
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

fn write_byte(addr: usize, value: u8) {
    unsafe { core::ptr::write_volatile(addr as *mut u8, value) }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    if verify_permission_matrix() {
        return 0;
    }

    let before = read_info();
    assert!(before.used_ids >= 0);

    let shmid = shmget(IPC_PRIVATE, SEGMENT_SIZE, IPC_CREAT | 0o640);
    assert!(shmid > 0, "shmget failed: {}", shmid);
    let shmid = shmid as usize;

    let initial = read_stat(shmid);
    assert_eq!(initial.shm_perm.key, IPC_PRIVATE as i32);
    assert_eq!(initial.shm_perm.uid, getuid() as u32);
    assert_eq!(initial.shm_perm.gid, getgid() as u32);
    assert_eq!(initial.shm_perm.cuid, getuid() as u32);
    assert_eq!(initial.shm_perm.cgid, getgid() as u32);
    assert_eq!(initial.shm_perm.mode & 0o777, 0o640);
    assert_eq!(initial.shm_segsz, SEGMENT_SIZE);
    assert_eq!(initial.shm_cpid, getpid() as i32);
    assert_eq!(initial.shm_lpid, 0);
    assert_eq!(initial.shm_nattch, 0);
    assert_eq!(initial.shm_atime, 0);
    assert_eq!(initial.shm_dtime, 0);

    let created = read_info();
    assert_eq!(created.used_ids, before.used_ids + 1);
    assert_eq!(created.shm_tot, before.shm_tot + 2);

    let index = find_index(shmid);
    let mut indexed = ShmidDs::default();
    let indexed_ptr = &mut indexed as *mut ShmidDs as usize;
    assert_eq!(shmctl(index, SHM_STAT, indexed_ptr), shmid as isize);
    assert_eq!(indexed.shm_segsz, SEGMENT_SIZE);
    let mut any = ShmidDs::default();
    let any_ptr = &mut any as *mut ShmidDs as usize;
    assert_eq!(shmctl(index, SHM_STAT_ANY, any_ptr), shmid as isize);
    assert_eq!(any.shm_cpid, getpid() as i32);

    let mapping = shmat(shmid, 0, 0);
    assert!(mapping > 0, "shmat failed: {}", mapping);
    let mapping = mapping as usize;
    write_byte(mapping, 0x51);
    write_byte(mapping + SEGMENT_SIZE - 1, 0xa7);

    let attached = read_stat(shmid);
    assert_eq!(attached.shm_nattch, 1);
    assert_eq!(attached.shm_lpid, getpid() as i32);
    assert!(attached.shm_atime >= initial.shm_ctime);

    assert_eq!(shmdt(mapping), 0);
    let detached = read_stat(shmid);
    assert_eq!(detached.shm_nattch, 0);
    assert_eq!(detached.shm_lpid, getpid() as i32);
    assert!(detached.shm_dtime >= attached.shm_atime);

    let mut update = detached;
    update.shm_perm.mode = 0o604;
    let update_ptr = &update as *const ShmidDs as usize;
    assert_eq!(shmctl(shmid, IPC_SET, update_ptr), 0);
    let updated = read_stat(shmid);
    assert_eq!(updated.shm_perm.mode & 0o777, 0o604);
    assert!(updated.shm_ctime >= detached.shm_ctime);

    let mapping = shmat(shmid, 0, 0);
    assert!(mapping > 0, "second shmat failed: {}", mapping);
    let mapping = mapping as usize;
    assert_eq!(read_byte(mapping), 0x51);
    assert_eq!(read_byte(mapping + SEGMENT_SIZE - 1), 0xa7);
    assert_eq!(shmctl(shmid, IPC_RMID, 0), 0);

    let removed = read_stat(shmid);
    assert_ne!(removed.shm_perm.mode & SHM_DEST, 0);
    assert_eq!(removed.shm_nattch, 1);
    let pending = read_info();
    assert_eq!(pending.used_ids, before.used_ids + 1);
    assert_eq!(pending.shm_tot, before.shm_tot + 2);

    assert_eq!(shmdt(mapping), 0);
    let mut gone = ShmidDs::default();
    let gone_ptr = &mut gone as *mut ShmidDs as usize;
    assert_eq!(shmctl(shmid, IPC_STAT, gone_ptr), -EINVAL);
    let after = read_info();
    assert_eq!(after.used_ids, before.used_ids);
    assert_eq!(after.shm_tot, before.shm_tot);

    println!(
        "SYSV_SHM_METADATA PASS index={} size={} pages=2 permission=pass",
        index, SEGMENT_SIZE
    );
    0
}
