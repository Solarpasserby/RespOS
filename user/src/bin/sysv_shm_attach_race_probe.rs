#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{fork, mmap_raw, munmap, shmat, shmctl, shmdt, shmget, waitpid, yield_};

const PAGE_SIZE: usize = 4096;
const IPC_PRIVATE: isize = 0;
const IPC_CREAT: usize = 0o1000;
const IPC_RMID: usize = 0;
const IPC_STAT: usize = 2;
const IPC_INFO: usize = 3;
const SHM_INFO: usize = 14;
const EINVAL: isize = 22;
const EIDRM: isize = 43;
const ENOSPC: isize = 28;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED_ANONYMOUS: usize = 0x1 | 0x20;
const ATTACHERS: usize = 2;
const ROUNDS: usize = 32;
const PRESSURE_ROUNDS: usize = 128;
const MAX_TRACKED_SEGMENTS: usize = 4096;
const CHILD_INVALID: i32 = 10;
const CHILD_ATTACHED: i32 = 11;
const CHILD_ORPHAN: i32 = 12;

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
struct RaceControl {
    ready: AtomicU32,
    go: AtomicU32,
}

fn removed_result(result: isize) -> bool {
    result == -EINVAL || result == -EIDRM
}

fn read_word(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn write_word(addr: usize, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

fn verify_shmmni_limit() -> usize {
    let mut limits = ShmInfoLimits::default();
    let limits_ptr = &mut limits as *mut ShmInfoLimits as usize;
    assert!(shmctl(0, IPC_INFO, limits_ptr) >= 0);

    let mut before = ShmInfo::default();
    let before_ptr = &mut before as *mut ShmInfo as usize;
    assert!(shmctl(0, SHM_INFO, before_ptr) >= 0);
    assert!(before.used_ids >= 0);
    let before_used = before.used_ids as usize;
    assert!(limits.shmmni > before_used);

    let available = limits.shmmni - before_used;
    assert!(available <= MAX_TRACKED_SEGMENTS);
    let mut ids = [0usize; MAX_TRACKED_SEGMENTS];
    for id_slot in ids[..available].iter_mut() {
        let id = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
        assert!(id > 0, "shmget before SHMMNI failed: {}", id);
        *id_slot = id as usize;
    }
    assert_eq!(shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600), -ENOSPC);

    let released = ids[available - 1];
    assert_eq!(shmctl(released, IPC_RMID, 0), 0);
    let replacement = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    assert!(
        replacement > 0,
        "replacement shmget failed: {}",
        replacement
    );
    ids[available - 1] = replacement as usize;

    for id in &ids[..available] {
        assert_eq!(shmctl(*id, IPC_RMID, 0), 0);
    }
    let mut after = ShmInfo::default();
    let after_ptr = &mut after as *mut ShmInfo as usize;
    assert!(shmctl(0, SHM_INFO, after_ptr) >= 0);
    assert_eq!(after.used_ids, before.used_ids);
    limits.shmmni
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let shmmni = verify_shmmni_limit();

    let control_addr = mmap_raw(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED_ANONYMOUS, -1, 0);
    assert!(control_addr > 0, "control mmap failed: {}", control_addr);
    let control_addr = control_addr as usize;
    let control = unsafe { &*(control_addr as *const RaceControl) };

    let rollback_id = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    assert!(rollback_id > 0, "rollback shmget failed: {}", rollback_id);
    let rollback_id = rollback_id as usize;
    let rollback_survivor = shmat(rollback_id, 0, 0);
    assert!(
        rollback_survivor > 0,
        "rollback survivor shmat failed: {}",
        rollback_survivor
    );
    assert_eq!(shmctl(rollback_id, IPC_RMID, 0), 0);
    assert_eq!(shmat(rollback_id, control_addr, 0), -EINVAL);
    assert_eq!(shmdt(rollback_survivor as usize), 0);
    assert_eq!(shmat(rollback_id, 0, 0), -EINVAL);

    for round in 0..PRESSURE_ROUNDS {
        let pressure_id = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
        assert!(pressure_id > 0, "pressure shmget failed: {}", pressure_id);
        let pressure_id = pressure_id as usize;
        let pressure = shmat(pressure_id, 0, 0);
        assert!(pressure > 0, "pressure shmat failed: {}", pressure);
        let pressure = pressure as usize;
        let value = round as u32 ^ 0x51a7;
        write_word(pressure, value);
        assert_eq!(shmctl(pressure_id, IPC_RMID, 0), 0);
        assert_eq!(read_word(pressure), value);
        assert_eq!(shmdt(pressure), 0);
        assert_eq!(shmat(pressure_id, 0, 0), -EINVAL);
    }

    let mut invalid = 0usize;
    let mut attached = 0usize;
    let mut orphan = 0usize;
    for _round in 0..ROUNDS {
        control.ready.store(0, Ordering::Relaxed);
        control.go.store(0, Ordering::Relaxed);

        let shmid = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
        assert!(shmid > 0, "shmget failed: {}", shmid);
        let shmid = shmid as usize;
        let parent = shmat(shmid, 0, 0);
        assert!(parent > 0, "parent shmat failed: {}", parent);
        let parent = parent as usize;
        write_word(parent, 0xa77a_c0de);
        assert_eq!(shmctl(shmid, IPC_RMID, 0), 0);

        let mut children = [0isize; ATTACHERS];
        for child_slot in children.iter_mut() {
            let child = fork();
            assert!(child >= 0, "fork failed: {}", child);
            if child == 0 {
                assert_eq!(shmdt(parent), 0);
                control.ready.fetch_add(1, Ordering::Release);
                while control.go.load(Ordering::Acquire) == 0 {
                    let _ = yield_();
                }

                let mapping = shmat(shmid, 0, 0);
                if removed_result(mapping) {
                    return CHILD_INVALID;
                }
                assert!(mapping > 0, "concurrent shmat failed: {}", mapping);
                let mapping = mapping as usize;
                assert_eq!(read_word(mapping), 0xa77a_c0de);

                let mut ds = ShmidDs::default();
                let ds_ptr = &mut ds as *mut ShmidDs as usize;
                let stat_result = shmctl(shmid, IPC_STAT, ds_ptr);
                if removed_result(stat_result) {
                    assert_eq!(shmdt(mapping), 0);
                    return CHILD_ORPHAN;
                }
                assert_eq!(stat_result, 0);
                assert!(ds.shm_nattch >= 1);
                assert_eq!(shmdt(mapping), 0);
                return CHILD_ATTACHED;
            }
            *child_slot = child;
        }

        while control.ready.load(Ordering::Acquire) != ATTACHERS as u32 {
            let _ = yield_();
        }
        control.go.store(1, Ordering::Release);
        let _ = yield_();
        let _ = yield_();
        assert_eq!(shmdt(parent), 0);

        for child in children {
            let mut status = 0;
            assert_eq!(waitpid(child as usize, &mut status), child);
            let code = status >> 8;
            match code {
                CHILD_INVALID => invalid += 1,
                CHILD_ATTACHED => attached += 1,
                CHILD_ORPHAN => orphan += 1,
                _ => panic!("unexpected child status: {}", status),
            }
        }
        assert_eq!(shmat(shmid, 0, 0), -EINVAL);
    }

    assert_eq!(invalid + attached + orphan, ROUNDS * ATTACHERS);
    assert_eq!(munmap(control_addr, PAGE_SIZE), 0);
    if orphan != 0 {
        println!(
            "SYSV_SHM_ATTACH_RACE_EXPECTED_FAIL orphan={} invalid={} attached={}",
            orphan, invalid, attached
        );
        return 1;
    }

    println!(
        "SYSV_SHM_ATTACH_RACE PASS shmmni={} pressure={} attempts={} invalid={} attached={}",
        shmmni,
        PRESSURE_ROUNDS,
        ROUNDS * ATTACHERS,
        invalid,
        attached
    );
    0
}
