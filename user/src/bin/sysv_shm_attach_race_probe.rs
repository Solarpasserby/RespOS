#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicU32, Ordering};
use user_lib::{
    O_WRONLY, close, fork, getpid, mmap_raw, munmap, open, shmat, shmctl, shmdt, shmget, waitpid,
    write, yield_,
};

const PAGE_SIZE: usize = 4096;
const IPC_PRIVATE: isize = 0;
const IPC_CREAT: usize = 0o1000;
const IPC_EXCL: usize = 0o2000;
const IPC_RMID: usize = 0;
const IPC_STAT: usize = 2;
const IPC_INFO: usize = 3;
const SHM_INFO: usize = 14;
const EINVAL: isize = 22;
const EIDRM: isize = 43;
const ENOSPC: isize = 28;
const EEXIST: isize = 17;
const ENOENT: isize = 2;
const EIO: isize = 5;
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

fn write_usize_sysctl(path: &str, mut value: usize) -> isize {
    let mut digits = [0u8; 32];
    let mut start = digits.len() - 1;
    digits[start] = b'\n';
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }

    let fd = open(path, O_WRONLY, 0);
    if fd < 0 {
        return fd;
    }
    let result = write(fd as usize, &digits[start..]);
    let close_result = close(fd as usize);
    if result == (digits.len() - start) as isize && close_result == 0 {
        0
    } else if result < 0 {
        result
    } else if close_result < 0 {
        close_result
    } else {
        -EIO
    }
}

fn remove_if_created(shmid: isize) -> isize {
    if shmid > 0 {
        shmctl(shmid as usize, IPC_RMID, 0)
    } else {
        0
    }
}

fn verify_size_limits() -> usize {
    let mut limits = ShmInfoLimits::default();
    let limits_ptr = &mut limits as *mut ShmInfoLimits as usize;
    assert!(shmctl(0, IPC_INFO, limits_ptr) >= 0);
    assert_eq!(limits.shmmin, 1);
    let oversized = limits.shmmax.checked_add(1).unwrap();

    assert_eq!(shmget(IPC_PRIVATE, 0, IPC_CREAT | 0o600), -EINVAL);
    assert_eq!(shmget(IPC_PRIVATE, oversized, IPC_CREAT | 0o600), -EINVAL);

    let mut key = 0x5200_0000isize ^ getpid();
    let mut keyed = -EEXIST;
    for _ in 0..256 {
        keyed = shmget(key, PAGE_SIZE, IPC_CREAT | IPC_EXCL | 0o600);
        if keyed > 0 {
            break;
        }
        assert_eq!(keyed, -EEXIST);
        key += 1;
    }
    assert!(keyed > 0, "keyed shmget failed: {}", keyed);
    let keyed = keyed as usize;

    assert_eq!(shmget(key, 0, 0), keyed as isize);
    assert_eq!(shmget(key, PAGE_SIZE, 0), keyed as isize);
    assert_eq!(shmget(key, PAGE_SIZE + 1, 0), -EINVAL);
    assert_eq!(shmget(key, 0, IPC_CREAT | IPC_EXCL | 0o600), -EEXIST);
    assert_eq!(shmctl(keyed, IPC_RMID, 0), 0);
    assert_eq!(shmget(key, 0, 0), -ENOENT);
    assert_eq!(shmget(key, 0, IPC_CREAT | IPC_EXCL | 0o600), -EINVAL);
    limits.shmmax
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

fn verify_shmall_limit() -> usize {
    let mut limits = ShmInfoLimits::default();
    let limits_ptr = &mut limits as *mut ShmInfoLimits as usize;
    assert!(shmctl(0, IPC_INFO, limits_ptr) >= 0);
    let original = limits.shmall;
    assert!(original >= 2);
    let reduce_result = write_usize_sysctl("/proc/sys/kernel/shmall\0", 2);

    let mut reduced = ShmInfoLimits::default();
    let reduced_ptr = &mut reduced as *mut ShmInfoLimits as usize;
    let reduced_info = shmctl(0, IPC_INFO, reduced_ptr);
    let first = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let second = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let overflow = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let first_remove = remove_if_created(first);
    let replacement = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let cleanup = [second, overflow, replacement].map(remove_if_created);

    let two_pages = shmget(IPC_PRIVATE, PAGE_SIZE * 2, IPC_CREAT | 0o600);
    let after_two_pages = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let two_page_cleanup = [two_pages, after_two_pages].map(remove_if_created);

    let restore_result = write_usize_sysctl("/proc/sys/kernel/shmall\0", original);
    let mut restored = ShmInfoLimits::default();
    let restored_ptr = &mut restored as *mut ShmInfoLimits as usize;
    let restored_info = shmctl(0, IPC_INFO, restored_ptr);

    assert_eq!(reduce_result, 0);
    assert!(reduced_info >= 0);
    assert_eq!(reduced.shmall, 2);
    assert!(first > 0 && second > 0);
    assert_eq!(overflow, -ENOSPC);
    assert_eq!(first_remove, 0);
    assert!(replacement > 0);
    assert!(cleanup.iter().all(|result| *result == 0));
    assert!(two_pages > 0);
    assert_eq!(after_two_pages, -ENOSPC);
    assert!(two_page_cleanup.iter().all(|result| *result == 0));
    assert_eq!(restore_result, 0);
    assert!(restored_info >= 0);
    assert_eq!(restored.shmall, original);
    original
}

fn verify_dynamic_limits() {
    let mut limits = ShmInfoLimits::default();
    let limits_ptr = &mut limits as *mut ShmInfoLimits as usize;
    assert!(shmctl(0, IPC_INFO, limits_ptr) >= 0);
    assert!(limits.shmall >= 2);
    assert!(limits.shmmni >= 2);

    let mut before = ShmInfo::default();
    let before_ptr = &mut before as *mut ShmInfo as usize;
    assert!(shmctl(0, SHM_INFO, before_ptr) >= 0);
    assert_eq!(before.used_ids, 0);
    assert_eq!(before.shm_tot, 0);

    let two_page_id = shmget(IPC_PRIVATE, PAGE_SIZE * 2, IPC_CREAT | 0o600);
    assert!(two_page_id > 0, "two-page shmget failed: {}", two_page_id);
    let lower_shmall = write_usize_sysctl("/proc/sys/kernel/shmall\0", 1);

    let mut lowered_shmall = ShmInfoLimits::default();
    let lowered_shmall_ptr = &mut lowered_shmall as *mut ShmInfoLimits as usize;
    let lowered_shmall_info = shmctl(0, IPC_INFO, lowered_shmall_ptr);
    let mut over_shmall = ShmInfo::default();
    let over_shmall_ptr = &mut over_shmall as *mut ShmInfo as usize;
    let over_shmall_info = shmctl(0, SHM_INFO, over_shmall_ptr);
    let mut two_page_ds = ShmidDs::default();
    let two_page_ds_ptr = &mut two_page_ds as *mut ShmidDs as usize;
    let existing_stat = shmctl(two_page_id as usize, IPC_STAT, two_page_ds_ptr);
    let existing_attach = shmat(two_page_id as usize, 0, 0);
    let mut existing_value = 0;
    let existing_detach = if existing_attach > 0 {
        write_word(existing_attach as usize, 0xd1a1_1e57);
        existing_value = read_word(existing_attach as usize);
        shmdt(existing_attach as usize)
    } else {
        0
    };
    let blocked_by_shmall = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let remove_two_page = remove_if_created(two_page_id);
    let at_shmall = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let blocked_at_shmall = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let shmall_cleanup = [blocked_by_shmall, at_shmall, blocked_at_shmall].map(remove_if_created);

    let restore_shmall = write_usize_sysctl("/proc/sys/kernel/shmall\0", limits.shmall);
    let mut restored_shmall = ShmInfoLimits::default();
    let restored_shmall_ptr = &mut restored_shmall as *mut ShmInfoLimits as usize;
    let restored_shmall_info = shmctl(0, IPC_INFO, restored_shmall_ptr);

    assert_eq!(lower_shmall, 0);
    assert!(lowered_shmall_info >= 0);
    assert_eq!(lowered_shmall.shmall, 1);
    assert!(over_shmall_info >= 0);
    assert_eq!(over_shmall.used_ids, 1);
    assert_eq!(over_shmall.shm_tot, 2);
    assert_eq!(existing_stat, 0);
    assert_eq!(two_page_ds.shm_segsz, PAGE_SIZE * 2);
    assert!(existing_attach > 0);
    assert_eq!(existing_value, 0xd1a1_1e57);
    assert_eq!(existing_detach, 0);
    assert_eq!(blocked_by_shmall, -ENOSPC);
    assert_eq!(remove_two_page, 0);
    assert!(at_shmall > 0);
    assert_eq!(blocked_at_shmall, -ENOSPC);
    assert!(shmall_cleanup.iter().all(|result| *result == 0));
    assert_eq!(restore_shmall, 0);
    assert!(restored_shmall_info >= 0);
    assert_eq!(restored_shmall.shmall, limits.shmall);

    let first = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let second = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    assert!(first > 0 && second > 0);
    let lower_shmmni = write_usize_sysctl("/proc/sys/kernel/shmmni\0", 1);

    let mut lowered_shmmni = ShmInfoLimits::default();
    let lowered_shmmni_ptr = &mut lowered_shmmni as *mut ShmInfoLimits as usize;
    let lowered_shmmni_info = shmctl(0, IPC_INFO, lowered_shmmni_ptr);
    let mut over_shmmni = ShmInfo::default();
    let over_shmmni_ptr = &mut over_shmmni as *mut ShmInfo as usize;
    let over_shmmni_info = shmctl(0, SHM_INFO, over_shmmni_ptr);
    let mut first_ds = ShmidDs::default();
    let first_ds_ptr = &mut first_ds as *mut ShmidDs as usize;
    let first_stat = shmctl(first as usize, IPC_STAT, first_ds_ptr);
    let mut second_ds = ShmidDs::default();
    let second_ds_ptr = &mut second_ds as *mut ShmidDs as usize;
    let second_stat = shmctl(second as usize, IPC_STAT, second_ds_ptr);
    let blocked_by_shmmni = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let remove_first = remove_if_created(first);
    let blocked_at_shmmni = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let remove_second = remove_if_created(second);
    let below_shmmni = shmget(IPC_PRIVATE, PAGE_SIZE, IPC_CREAT | 0o600);
    let shmmni_cleanup =
        [blocked_by_shmmni, blocked_at_shmmni, below_shmmni].map(remove_if_created);

    let restore_shmmni = write_usize_sysctl("/proc/sys/kernel/shmmni\0", limits.shmmni);
    let mut restored_shmmni = ShmInfoLimits::default();
    let restored_shmmni_ptr = &mut restored_shmmni as *mut ShmInfoLimits as usize;
    let restored_shmmni_info = shmctl(0, IPC_INFO, restored_shmmni_ptr);

    assert_eq!(lower_shmmni, 0);
    assert!(lowered_shmmni_info >= 0);
    assert_eq!(lowered_shmmni.shmmni, 1);
    assert!(over_shmmni_info >= 0);
    assert_eq!(over_shmmni.used_ids, 2);
    assert_eq!(first_stat, 0);
    assert_eq!(first_ds.shm_segsz, PAGE_SIZE);
    assert_eq!(second_stat, 0);
    assert_eq!(second_ds.shm_segsz, PAGE_SIZE);
    assert_eq!(blocked_by_shmmni, -ENOSPC);
    assert_eq!(remove_first, 0);
    assert_eq!(blocked_at_shmmni, -ENOSPC);
    assert_eq!(remove_second, 0);
    assert!(below_shmmni > 0);
    assert!(shmmni_cleanup.iter().all(|result| *result == 0));
    assert_eq!(restore_shmmni, 0);
    assert!(restored_shmmni_info >= 0);
    assert_eq!(restored_shmmni.shmmni, limits.shmmni);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let shmmax = verify_size_limits();
    let shmmni = verify_shmmni_limit();
    let shmall = verify_shmall_limit();
    verify_dynamic_limits();

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
        "SYSV_SHM_ATTACH_RACE PASS shmmax={} shmmni={} shmall={} dynamic_limits=pass pressure={} attempts={} invalid={} attached={}",
        shmmax,
        shmmni,
        shmall,
        PRESSURE_ROUNDS,
        ROUNDS * ATTACHERS,
        invalid,
        attached
    );
    0
}
