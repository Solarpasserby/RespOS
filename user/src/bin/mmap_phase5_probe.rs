#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDWR, O_TRUNC, Stat, close, exit, fadvise64, fallocate, fork, fstat, fsync,
    ftruncate, lseek, mmap, msync, munmap, open, pipe, pread, pwrite, read, unlink, waitpid, write,
};

const PATH: &str = "/tmp/respos_mmap_phase5.tmp\0";
const ENOSPC_TARGET_PATH: &str = "/respos/respos_mmap_enospc_target.tmp\0";
const ENOSPC_FILLER_PATH: &str = "/respos/respos_mmap_enospc_filler.tmp\0";
const PAGE_SIZE: usize = 4096;
const MAP_SIZE: usize = 3 * PAGE_SIZE;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_SHARED: usize = 0x1;
const MAP_PRIVATE: usize = 0x2;
const SIGBUS: i32 = 7;
const FALLOC_FL_KEEP_SIZE: usize = 0x01;
const FALLOC_FL_PUNCH_HOLE: usize = 0x02;
const SEEK_SET: usize = 0;
const SEEK_CUR: usize = 1;
const POSIX_FADV_DONTNEED: usize = 4;
const MS_SYNC: i32 = 4;

fn pipe_send(fd: i32) {
    assert_eq!(write(fd as usize, &[1]), 1);
}

fn pipe_recv(fd: i32) {
    let mut byte = [0u8; 1];
    assert_eq!(read(fd as usize, &mut byte), 1);
}

fn close_pipe(pipefd: [i32; 2]) {
    assert_eq!(close(pipefd[0] as usize), 0);
    assert_eq!(close(pipefd[1] as usize), 0);
}

fn wait_child(pid: isize) -> Option<i32> {
    let mut status = -1;
    let result = waitpid(pid as usize, &mut status);
    if result != pid {
        println!("MMAP_PHASE5 waitpid pid={} result={}", pid, result);
        None
    } else {
        Some(status)
    }
}

fn expect_sigbus(address: usize, label: &str, name: &str) -> bool {
    let child = fork();
    if child < 0 {
        println!("MMAP_PHASE5 {} {} fork failed={}", label, name, child);
        return false;
    }
    if child == 0 {
        let value = unsafe { (address as *const u8).read_volatile() };
        core::hint::black_box(value);
        exit(99);
        unreachable!();
    }
    let Some(status) = wait_child(child) else {
        return false;
    };
    if status & 0x7f != SIGBUS {
        println!(
            "MMAP_PHASE5_EXPECTED_FAIL {} {} status={} expected_signal={}",
            label, name, status, SIGBUS
        );
        false
    } else {
        true
    }
}

fn expect_byte(address: usize, expected: u8, label: &str, name: &str) -> bool {
    let child = fork();
    if child < 0 {
        println!("MMAP_PHASE5 {} {} fork failed={}", label, name, child);
        return false;
    }
    if child == 0 {
        let actual = unsafe { (address as *const u8).read_volatile() };
        exit(if actual == expected { 0 } else { 98 });
        unreachable!();
    }
    let Some(status) = wait_child(child) else {
        return false;
    };
    if status != 0 {
        println!(
            "MMAP_PHASE5_EXPECTED_FAIL {} {} status={} expected_byte={:#x}",
            label, name, status, expected
        );
        false
    } else {
        true
    }
}

fn write_byte(fd: usize, offset: usize, value: u8) -> bool {
    pwrite(fd, &[value], offset as isize) == 1
}

fn test_mode(fd: usize, map_flag: usize, label: &str) -> bool {
    let mut ok = true;
    assert_eq!(ftruncate(fd, PAGE_SIZE + 128), 0);
    assert!(write_byte(fd, PAGE_SIZE + 64, 0x5a));
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, map_flag, fd as isize, 0);
    assert!(mapping > 0);
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 64,
        0x5a,
        label,
        "initial_data",
    );
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 512,
        0,
        label,
        "partial_eof_zero",
    );
    ok &= expect_sigbus(
        mapping as usize + 2 * PAGE_SIZE,
        label,
        "initial_beyond_eof",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);

    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    assert!(write_byte(fd, PAGE_SIZE + 512, 0x7c));
    assert!(write_byte(fd, 2 * PAGE_SIZE + 17, 0x6d));
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, map_flag, fd as isize, 0);
    assert!(mapping > 0);
    let partial_before =
        unsafe { ((mapping as usize + PAGE_SIZE + 512) as *const u8).read_volatile() };
    let full_before =
        unsafe { ((mapping as usize + 2 * PAGE_SIZE + 17) as *const u8).read_volatile() };
    assert_eq!(partial_before, 0x7c);
    assert_eq!(full_before, 0x6d);
    assert_eq!(ftruncate(fd, PAGE_SIZE + 128), 0);
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 512,
        0,
        label,
        "truncate_partial_zero",
    );
    ok &= expect_sigbus(
        mapping as usize + 2 * PAGE_SIZE + 17,
        label,
        "truncate_resident_beyond_eof",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);

    assert_eq!(ftruncate(fd, PAGE_SIZE), 0);
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, map_flag, fd as isize, 0);
    assert!(mapping > 0);
    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    assert!(write_byte(fd, 2 * PAGE_SIZE + 33, 0xa7));
    ok &= expect_byte(
        mapping as usize + 2 * PAGE_SIZE + 33,
        0xa7,
        label,
        "growth_dynamic_eof",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);

    if ok {
        println!("MMAP_PHASE5 {} PASS", label);
    }
    ok
}

fn test_private_cow_truncate(fd: usize) -> bool {
    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_PRIVATE, fd as isize, 0);
    assert!(mapping > 0);
    unsafe {
        ((mapping as usize + PAGE_SIZE + 64) as *mut u8).write_volatile(0x44);
        ((mapping as usize + PAGE_SIZE + 512) as *mut u8).write_volatile(0x55);
        ((mapping as usize + 2 * PAGE_SIZE + 17) as *mut u8).write_volatile(0x66);
    }
    assert_eq!(ftruncate(fd, PAGE_SIZE + 128), 0);
    let mut ok = true;
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 64,
        0x44,
        "private",
        "cow_retained_data",
    );
    ok &= expect_byte(
        mapping as usize + PAGE_SIZE + 512,
        0x55,
        "private",
        "cow_partial_tail_retained",
    );
    ok &= expect_sigbus(
        mapping as usize + 2 * PAGE_SIZE + 17,
        "private",
        "cow_full_page_sigbus",
    );
    assert_eq!(munmap(mapping as usize, MAP_SIZE), 0);
    if ok {
        println!("MMAP_PHASE5 private_cow_truncate PASS");
    }
    ok
}

fn spawn_truncate_mapper(fd: usize, ready_fd: i32, go_fd: i32, prefault: bool) -> isize {
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_SHARED, fd as isize, 0);
        assert!(mapping > 0);
        if prefault {
            let value =
                unsafe { ((mapping as usize + 2 * PAGE_SIZE + 17) as *const u8).read_volatile() };
            core::hint::black_box(value);
        }
        pipe_send(ready_fd);
        pipe_recv(go_fd);
        let value =
            unsafe { ((mapping as usize + 2 * PAGE_SIZE + 17) as *const u8).read_volatile() };
        core::hint::black_box(value);
        exit(99);
        unreachable!();
    }
    child
}

fn expect_child_sigbus(child: isize, label: &str) -> bool {
    let Some(status) = wait_child(child) else {
        return false;
    };
    if status & 0x7f != SIGBUS {
        println!(
            "MMAP_PHASE5_EXPECTED_FAIL {} status={} expected_signal={}",
            label, status, SIGBUS
        );
        false
    } else {
        true
    }
}

fn test_cross_process_truncate(fd: usize) -> bool {
    let mut ready = [0; 2];
    let mut resident_go = [0; 2];
    let mut unfaulted_go = [0; 2];
    let mut truncate_go = [0; 2];
    let mut truncate_done = [0; 2];
    assert_eq!(pipe(&mut ready), 0);
    assert_eq!(pipe(&mut resident_go), 0);
    assert_eq!(pipe(&mut unfaulted_go), 0);
    assert_eq!(pipe(&mut truncate_go), 0);
    assert_eq!(pipe(&mut truncate_done), 0);
    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    assert!(write_byte(fd, 2 * PAGE_SIZE + 17, 0x81));

    let resident = spawn_truncate_mapper(fd, ready[1], resident_go[0], true);
    let unfaulted = spawn_truncate_mapper(fd, ready[1], unfaulted_go[0], false);
    let truncator = fork();
    assert!(truncator >= 0);
    if truncator == 0 {
        pipe_recv(truncate_go[0]);
        assert_eq!(ftruncate(fd, PAGE_SIZE), 0);
        pipe_send(truncate_done[1]);
        exit(0);
        unreachable!();
    }

    pipe_recv(ready[0]);
    pipe_recv(ready[0]);
    pipe_send(truncate_go[1]);
    pipe_recv(truncate_done[0]);
    pipe_send(resident_go[1]);
    pipe_send(unfaulted_go[1]);
    let mut ok = expect_child_sigbus(resident, "cross_process_resident");
    ok &= expect_child_sigbus(unfaulted, "cross_process_unfaulted");
    let Some(status) = wait_child(truncator) else {
        return false;
    };
    ok &= status == 0;
    close_pipe(ready);
    close_pipe(resident_go);
    close_pipe(unfaulted_go);
    close_pipe(truncate_go);
    close_pipe(truncate_done);
    if ok {
        println!("MMAP_PHASE5 cross_process_truncate PASS");
    }
    ok
}

fn test_cross_process_fault_store_race(fd: usize) -> bool {
    const ROUNDS: usize = 16;
    for round in 0..ROUNDS {
        let mut ready = [0; 2];
        let mut mapper_go = [0; 2];
        let mut truncate_go = [0; 2];
        let mut truncate_done = [0; 2];
        assert_eq!(pipe(&mut ready), 0);
        assert_eq!(pipe(&mut mapper_go), 0);
        assert_eq!(pipe(&mut truncate_go), 0);
        assert_eq!(pipe(&mut truncate_done), 0);
        assert_eq!(ftruncate(fd, MAP_SIZE), 0);

        let mapper = fork();
        assert!(mapper >= 0);
        if mapper == 0 {
            let mapping = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_SHARED, fd as isize, 0);
            assert!(mapping > 0);
            pipe_send(ready[1]);
            pipe_recv(mapper_go[0]);
            unsafe {
                ((mapping as usize + 2 * PAGE_SIZE + 31) as *mut u8).write_volatile(round as u8);
            }
            pipe_recv(truncate_done[0]);
            unsafe {
                let address = (mapping as usize + 2 * PAGE_SIZE + 31) as *mut u8;
                address.write_volatile(address.read_volatile() ^ 1);
            }
            exit(99);
            unreachable!();
        }

        let truncator = fork();
        assert!(truncator >= 0);
        if truncator == 0 {
            pipe_recv(truncate_go[0]);
            assert_eq!(ftruncate(fd, PAGE_SIZE), 0);
            pipe_send(truncate_done[1]);
            exit(0);
            unreachable!();
        }

        pipe_recv(ready[0]);
        pipe_send(mapper_go[1]);
        pipe_send(truncate_go[1]);
        if !expect_child_sigbus(mapper, "cross_process_fault_store_race") {
            return false;
        }
        let Some(status) = wait_child(truncator) else {
            return false;
        };
        if status != 0 {
            println!(
                "MMAP_PHASE5_EXPECTED_FAIL truncate_race round={} status={}",
                round, status
            );
            return false;
        }
        close_pipe(ready);
        close_pipe(mapper_go);
        close_pipe(truncate_go);
        close_pipe(truncate_done);
    }
    println!("MMAP_PHASE5 cross_process_fault_store_race PASS");
    true
}

fn expect_zero_range(fd: usize, mut offset: usize, mut len: usize) {
    let mut buffer = [0u8; PAGE_SIZE];
    while len != 0 {
        let count = len.min(buffer.len());
        assert_eq!(
            pread(fd, &mut buffer[..count], offset as isize),
            count as isize
        );
        assert!(buffer[..count].iter().all(|byte| *byte == 0));
        offset += count;
        len -= count;
    }
}

fn test_punch_hole(fd: usize) -> bool {
    const PUNCH_START: usize = PAGE_SIZE / 2;
    const PUNCH_LEN: usize = 2 * PAGE_SIZE;
    const PUNCH_END: usize = PUNCH_START + PUNCH_LEN;

    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    let mut page = [0u8; PAGE_SIZE];
    for (index, byte) in page.iter_mut().enumerate() {
        *byte = (index % 251 + 1) as u8;
    }
    for offset in (0..MAP_SIZE).step_by(PAGE_SIZE) {
        assert_eq!(pwrite(fd, &page, offset as isize), PAGE_SIZE as isize);
    }
    assert_eq!(fsync(fd), 0);

    let shared = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_SHARED, fd as isize, 0);
    let clean = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_PRIVATE, fd as isize, 0);
    let cow = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_PRIVATE, fd as isize, 0);
    assert!(shared > 0 && clean > 0 && cow > 0);
    unsafe {
        core::hint::black_box(((shared as usize + PUNCH_START + 17) as *const u8).read_volatile());
        core::hint::black_box(((shared as usize + PAGE_SIZE + 17) as *const u8).read_volatile());
        core::hint::black_box(((clean as usize + PUNCH_START + 17) as *const u8).read_volatile());
        core::hint::black_box(((clean as usize + PAGE_SIZE + 17) as *const u8).read_volatile());
        ((cow as usize + PAGE_SIZE + 17) as *mut u8).write_volatile(0xa5);
        ((cow as usize + 2 * PAGE_SIZE + 17) as *mut u8).write_volatile(0xb6);
    }

    let mut before = Stat::default();
    let mut after = Stat::default();
    assert_eq!(fstat(fd, &mut before), 0);
    assert_eq!(lseek(fd, 123, SEEK_SET), 123);
    assert_eq!(
        fallocate(
            fd,
            FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
            PUNCH_START as isize,
            PUNCH_LEN as isize,
        ),
        0
    );
    assert_eq!(lseek(fd, 0, SEEK_CUR), 123);
    assert_eq!(fstat(fd, &mut after), 0);
    assert_eq!(after.st_size, MAP_SIZE as u64);
    println!(
        "MMAP_PHASE5 punch_hole blocks={}->{}",
        before.st_blocks, after.st_blocks
    );
    assert!(after.st_blocks < before.st_blocks);
    expect_zero_range(fd, PUNCH_START, PUNCH_LEN);

    let mut ok = true;
    for (mapping, label) in [(shared, "shared"), (clean, "private_clean")] {
        ok &= expect_byte(mapping as usize + PUNCH_START + 17, 0, "punch_hole", label);
        ok &= expect_byte(mapping as usize + PAGE_SIZE + 17, 0, "punch_hole", label);
        ok &= expect_byte(
            mapping as usize + 2 * PAGE_SIZE + 17,
            0,
            "punch_hole",
            label,
        );
    }
    ok &= expect_byte(
        cow as usize + PAGE_SIZE + 17,
        0xa5,
        "punch_hole",
        "private_cow_full",
    );
    ok &= expect_byte(
        cow as usize + 2 * PAGE_SIZE + 17,
        0xb6,
        "punch_hole",
        "private_cow_partial",
    );
    ok &= expect_byte(
        shared as usize + PUNCH_START - 1,
        page[(PUNCH_START - 1) % PAGE_SIZE],
        "punch_hole",
        "left_boundary",
    );
    ok &= expect_byte(
        shared as usize + PUNCH_END,
        page[PUNCH_END % PAGE_SIZE],
        "punch_hole",
        "right_boundary",
    );
    assert_eq!(munmap(shared as usize, MAP_SIZE), 0);
    assert_eq!(munmap(clean as usize, MAP_SIZE), 0);
    assert_eq!(munmap(cow as usize, MAP_SIZE), 0);
    assert_eq!(fsync(fd), 0);
    assert_eq!(fadvise64(fd, 0, MAP_SIZE as isize, POSIX_FADV_DONTNEED), 0);
    expect_zero_range(fd, PUNCH_START, PUNCH_LEN);
    if ok {
        println!("MMAP_PHASE5 punch_hole PASS");
    }
    ok
}

fn test_cross_process_punch_hole(fd: usize) -> bool {
    assert_eq!(ftruncate(fd, MAP_SIZE), 0);
    let page = [0x6du8; PAGE_SIZE];
    for offset in (0..MAP_SIZE).step_by(PAGE_SIZE) {
        assert_eq!(pwrite(fd, &page, offset as isize), PAGE_SIZE as isize);
    }
    assert_eq!(fsync(fd), 0);

    let mut ready = [0; 2];
    let mut go = [0; 2];
    assert_eq!(pipe(&mut ready), 0);
    assert_eq!(pipe(&mut go), 0);
    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        let shared = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_SHARED, fd as isize, 0);
        let clean = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_PRIVATE, fd as isize, 0);
        let cow = mmap(0, MAP_SIZE, PROT_READ_WRITE, MAP_PRIVATE, fd as isize, 0);
        assert!(shared > 0 && clean > 0 && cow > 0);
        unsafe {
            core::hint::black_box(
                ((shared as usize + PAGE_SIZE + 31) as *const u8).read_volatile(),
            );
            core::hint::black_box(((clean as usize + PAGE_SIZE + 31) as *const u8).read_volatile());
            ((cow as usize + PAGE_SIZE + 31) as *mut u8).write_volatile(0xa5);
        }
        pipe_send(ready[1]);
        pipe_recv(go[0]);
        let shared_value =
            unsafe { ((shared as usize + PAGE_SIZE + 31) as *const u8).read_volatile() };
        let clean_value =
            unsafe { ((clean as usize + PAGE_SIZE + 31) as *const u8).read_volatile() };
        let cow_value = unsafe { ((cow as usize + PAGE_SIZE + 31) as *const u8).read_volatile() };
        exit(
            if shared_value == 0 && clean_value == 0 && cow_value == 0xa5 {
                0
            } else {
                98
            },
        );
        unreachable!();
    }

    pipe_recv(ready[0]);
    assert_eq!(
        fallocate(
            fd,
            FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
            PAGE_SIZE as isize,
            PAGE_SIZE as isize,
        ),
        0
    );
    pipe_send(go[1]);
    let ok = wait_child(child).is_some_and(|status| status == 0);
    close_pipe(ready);
    close_pipe(go);
    if ok {
        println!("MMAP_PHASE5 cross_process_punch_hole PASS");
    }
    ok
}

fn test_punch_hole_errors(fd: usize) -> bool {
    const EINVAL: isize = 22;
    const EOPNOTSUPP: isize = 95;

    let mut before = Stat::default();
    let mut after = Stat::default();
    assert_eq!(fstat(fd, &mut before), 0);
    assert_eq!(
        fallocate(fd, FALLOC_FL_PUNCH_HOLE, 0, PAGE_SIZE as isize),
        -EOPNOTSUPP
    );
    assert_eq!(
        fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, 0, 0),
        -EINVAL
    );
    assert_eq!(
        fallocate(
            fd,
            FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
            -1,
            PAGE_SIZE as isize,
        ),
        -EINVAL
    );
    assert_eq!(
        fallocate(
            fd,
            FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
            (MAP_SIZE + PAGE_SIZE) as isize,
            PAGE_SIZE as isize,
        ),
        0
    );
    assert_eq!(fstat(fd, &mut after), 0);
    assert_eq!(after.st_size, before.st_size);
    assert_eq!(after.st_blocks, before.st_blocks);
    println!("MMAP_PHASE5 punch_hole_errors PASS");
    true
}

fn test_punch_msync_race(fd: usize) -> bool {
    const ROUNDS: usize = 16;
    for round in 0..ROUNDS {
        assert_eq!(ftruncate(fd, PAGE_SIZE), 0);
        let page = [0x6du8; PAGE_SIZE];
        assert_eq!(pwrite(fd, &page, 0), PAGE_SIZE as isize);
        assert_eq!(fsync(fd), 0);

        let mut ready = [0; 2];
        let mut go = [0; 2];
        assert_eq!(pipe(&mut ready), 0);
        assert_eq!(pipe(&mut go), 0);
        let mapper = fork();
        assert!(mapper >= 0);
        if mapper == 0 {
            let mapping = mmap(0, PAGE_SIZE, PROT_READ_WRITE, MAP_SHARED, fd as isize, 0);
            assert!(mapping > 0);
            pipe_send(ready[1]);
            pipe_recv(go[0]);
            unsafe {
                ((mapping as usize + 31) as *mut u8).write_volatile((round + 1) as u8);
            }
            assert_eq!(msync(mapping as usize, PAGE_SIZE, MS_SYNC), 0);
            exit(0);
            unreachable!();
        }

        let puncher = fork();
        assert!(puncher >= 0);
        if puncher == 0 {
            pipe_recv(go[0]);
            assert_eq!(
                fallocate(
                    fd,
                    FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
                    0,
                    PAGE_SIZE as isize,
                ),
                0
            );
            exit(0);
            unreachable!();
        }

        pipe_recv(ready[0]);
        pipe_send(go[1]);
        pipe_send(go[1]);
        assert_eq!(wait_child(mapper), Some(0));
        assert_eq!(wait_child(puncher), Some(0));
        let mut byte = [0u8; 1];
        assert_eq!(pread(fd, &mut byte, 31), 1);
        assert!(byte[0] == 0 || byte[0] == (round + 1) as u8);
        close_pipe(ready);
        close_pipe(go);
    }
    println!("MMAP_PHASE5 punch_msync_race PASS");
    true
}

fn test_shared_write_enospc_sigbus() -> bool {
    const ENOSPC: isize = 28;
    const FILL_CHUNK: usize = 16 * PAGE_SIZE;
    const MAX_FILL_CHUNKS: usize = 1024;

    let _ = unlink(ENOSPC_TARGET_PATH);
    let _ = unlink(ENOSPC_FILLER_PATH);
    let target = open(ENOSPC_TARGET_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(target >= 0);
    let target = target as usize;
    assert_eq!(ftruncate(target, PAGE_SIZE), 0);
    assert_eq!(fsync(target), 0);

    let filler = open(ENOSPC_FILLER_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(filler >= 0);
    let filler = filler as usize;
    let fill = [0x5au8; FILL_CHUNK];
    let mut reached_enospc = false;
    for chunk in 0..MAX_FILL_CHUNKS {
        assert_eq!(
            pwrite(filler, &fill, (chunk * FILL_CHUNK) as isize),
            FILL_CHUNK as isize
        );
        let sync_result = fsync(filler);
        if sync_result == -ENOSPC {
            reached_enospc = true;
            break;
        }
        assert_eq!(sync_result, 0);
    }
    assert!(reached_enospc);

    let child = fork();
    assert!(child >= 0);
    if child == 0 {
        let mapping = mmap(
            0,
            PAGE_SIZE,
            PROT_READ_WRITE,
            MAP_SHARED,
            target as isize,
            0,
        );
        assert!(mapping > 0);
        unsafe {
            (mapping as *mut u8).write_volatile(0xa5);
        }
        exit(99);
        unreachable!();
    }
    let ok = expect_child_sigbus(child, "shared_write_enospc");

    // Drop the failed dirty tail before unlinking so the disposable guest can
    // complete its normal teardown without a permanent full-filesystem state.
    assert_eq!(ftruncate(filler, 0), 0);
    assert_eq!(close(filler), 0);
    assert_eq!(unlink(ENOSPC_FILLER_PATH), 0);

    let mapping = mmap(
        0,
        PAGE_SIZE,
        PROT_READ_WRITE,
        MAP_SHARED,
        target as isize,
        0,
    );
    assert!(mapping > 0);
    unsafe {
        (mapping as *mut u8).write_volatile(0x3c);
    }
    assert_eq!(munmap(mapping as usize, PAGE_SIZE), 0);
    assert_eq!(fsync(target), 0);
    let mut recovered = [0u8; 1];
    assert_eq!(pread(target, &mut recovered, 0), 1);
    assert_eq!(recovered[0], 0x3c);
    assert_eq!(close(target), 0);
    assert_eq!(unlink(ENOSPC_TARGET_PATH), 0);
    if ok {
        println!("MMAP_PHASE5 shared_write_enospc_sigbus PASS");
    }
    ok
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0);
    let fd = fd as usize;

    let shared_ok = test_mode(fd, MAP_SHARED, "shared");
    assert_eq!(ftruncate(fd, 0), 0);
    let private_ok = test_mode(fd, MAP_PRIVATE, "private");
    let private_cow_ok = test_private_cow_truncate(fd);
    let cross_process_ok = test_cross_process_truncate(fd);
    let race_ok = test_cross_process_fault_store_race(fd);
    let punch_ok = test_punch_hole(fd);
    let cross_process_punch_ok = test_cross_process_punch_hole(fd);
    let punch_errors_ok = test_punch_hole_errors(fd);
    let punch_race_ok = test_punch_msync_race(fd);
    let enospc_ok = test_shared_write_enospc_sigbus();

    assert_eq!(close(fd), 0);
    assert_eq!(unlink(PATH), 0);
    if shared_ok
        && private_ok
        && private_cow_ok
        && cross_process_ok
        && race_ok
        && punch_ok
        && cross_process_punch_ok
        && punch_errors_ok
        && punch_race_ok
        && enospc_ok
    {
        println!("MMAP_PHASE5 ALL PASS");
        0
    } else {
        println!("MMAP_PHASE5 CURRENT DIFFERENCES CONFIRMED");
        1
    }
}
