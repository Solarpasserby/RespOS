#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

extern crate alloc;

use alloc::format;

use user_lib::{
    O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, close, dup, fdatasync, fstat, fsync, mkdir,
    mmap, mount, msync, munmap, open, pipe, pread, pwrite, rmdir, sync, sync_file_range, syncfs,
    umount2, unlink, write,
};

const TARGET: &str = "/respos-writeback-target\0";
const FAULT_CONTROL: &str = "/proc/respos_perf\0";
const EIO: isize = -5;
const EINVAL: isize = -22;
const PAGE_SIZE: usize = 4096;
const SYNC_FILE_RANGE_WAIT_AFTER: usize = 4;
const SYNC_FILE_RANGE_WRITE: usize = 2;
const MS_INVALIDATE: i32 = 2;
const MS_SYNC: i32 = 4;

fn cleanup() {
    let _ = unlink(TARGET);
}

fn normal() {
    cleanup();
    let writer = open(TARGET, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(writer >= 0);
    let writer = writer as usize;
    let observer = open(TARGET, O_RDONLY, 0);
    assert!(observer >= 0);
    let observer = observer as usize;

    let payload = b"writeback-visible-through-shared-page-cache";
    assert_eq!(write(writer, payload), payload.len() as isize);
    let mut writer_stat = user_lib::Stat::default();
    let mut observer_stat = user_lib::Stat::default();
    assert_eq!(fstat(writer, &mut writer_stat), 0);
    assert_eq!(fstat(observer, &mut observer_stat), 0);
    assert_eq!(observer_stat.st_mtime, writer_stat.st_mtime);
    // fsync/fdatasync do not require a writable descriptor. A separately
    // opened descriptor for the same inode must flush the shared PageCache.
    assert_eq!(fdatasync(observer), 0);
    let mut actual = [0u8; 43];
    assert_eq!(pread(observer, &mut actual, 0), payload.len() as isize);
    assert_eq!(&actual[..payload.len()], payload);
    assert_eq!(fsync(writer), 0);

    let mut pipe_fds = [0i32; 2];
    assert_eq!(pipe(&mut pipe_fds), 0);
    assert_eq!(fsync(pipe_fds[0] as usize), EINVAL);
    assert_eq!(syncfs(pipe_fds[0] as usize), EINVAL);
    assert_eq!(close(pipe_fds[0] as usize), 0);
    assert_eq!(close(pipe_fds[1] as usize), 0);
    assert_eq!(close(observer), 0);
    assert_eq!(close(writer), 0);
    assert_eq!(unlink(TARGET), 0);
    println!("FS_WRITEBACK_PROBE_PASS");
}

fn phase3() {
    const PATH: &str = "/respos-phase3-sync.bin\0";
    const GLOBAL_PATH: &str = "/respos-phase3-global.bin\0";
    const MOUNT_PATH: &str = "/respos-phase3-mount\0";
    const MOUNT_FILE: &str = "/respos-phase3-mount/dirty.bin\0";
    let _ = unlink(PATH);
    let _ = unlink(GLOBAL_PATH);
    let _ = rmdir(MOUNT_PATH);

    let writer = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(writer >= 0);
    let writer = writer as usize;
    assert_eq!(pwrite(writer, &[0x31; 32], 0), 32);
    assert_eq!(pwrite(writer, &[0x72; 32], PAGE_SIZE as isize), 32);
    // Closing the final descriptor must leave the inode owned by writeback,
    // not synchronously force the lower filesystem.
    assert_eq!(close(writer), 0);

    let observer = open(PATH, O_RDONLY, 0);
    assert!(observer >= 0);
    let observer = observer as usize;
    assert_eq!(
        sync_file_range(
            observer,
            PAGE_SIZE as isize,
            PAGE_SIZE as isize,
            SYNC_FILE_RANGE_WRITE | SYNC_FILE_RANGE_WAIT_AFTER,
        ),
        0
    );

    let mapped = mmap(0, PAGE_SIZE * 2, 0x3, 0x1, observer as isize, 0);
    // A writable MAP_SHARED mapping needs a writable fd.
    assert!(mapped < 0);
    assert_eq!(close(observer), 0);

    let writer = open(PATH, O_RDWR, 0);
    assert!(writer >= 0);
    let writer = writer as usize;
    let mapped = mmap(0, PAGE_SIZE * 2, 0x3, 0x1, writer as isize, 0);
    assert!(mapped > 0);
    unsafe { (mapped as *mut u8).write_volatile(0xa4) };
    assert_eq!(
        msync(mapped as usize, PAGE_SIZE, MS_SYNC | MS_INVALIDATE),
        0
    );
    assert_eq!(munmap(mapped as usize, PAGE_SIZE * 2), 0);
    assert_eq!(syncfs(writer), 0);
    assert_eq!(close(writer), 0);

    let verify = open(PATH, O_RDONLY, 0);
    assert!(verify >= 0);
    let verify = verify as usize;
    let mut first = [0u8; 1];
    let mut second = [0u8; 32];
    assert_eq!(pread(verify, &mut first, 0), 1);
    assert_eq!(first[0], 0xa4);
    assert_eq!(pread(verify, &mut second, PAGE_SIZE as isize), 32);
    assert!(second.iter().all(|byte| *byte == 0x72));
    assert_eq!(close(verify), 0);

    let global = open(GLOBAL_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(global >= 0);
    let global = global as usize;
    assert_eq!(write(global, b"global-sync-owner"), 17);
    assert_eq!(close(global), 0);
    assert_eq!(sync(), 0);

    // Cross the dirty-owner threshold with many one-page files, then verify
    // the bounded safe-point writer and final global drain keep all contents.
    for index in 0..132usize {
        let path = format!("/respos-phase3-owner-{}\0", index);
        let fd = open(path.as_str(), O_CREATE | O_TRUNC | O_RDWR, 0o600);
        assert!(fd >= 0);
        let fd = fd as usize;
        assert_eq!(write(fd, &[index as u8]), 1);
        assert_eq!(close(fd), 0);
    }
    assert_eq!(sync(), 0);
    for index in 0..132usize {
        let path = format!("/respos-phase3-owner-{}\0", index);
        let fd = open(path.as_str(), O_RDONLY, 0);
        assert!(fd >= 0);
        let fd = fd as usize;
        let mut value = [0u8; 1];
        assert_eq!(pread(fd, &mut value, 0), 1);
        assert_eq!(value[0], index as u8);
        assert_eq!(close(fd), 0);
        assert_eq!(unlink(path.as_str()), 0);
    }

    assert_eq!(mkdir(MOUNT_PATH, 0o755), 0);
    assert_eq!(mount("\0", MOUNT_PATH, "tmpfs\0", 0, 0), 0);
    let mounted = open(MOUNT_FILE, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(mounted >= 0);
    let mounted = mounted as usize;
    assert_eq!(write(mounted, b"dirty-before-unmount"), 20);
    assert_eq!(close(mounted), 0);
    assert_eq!(umount2(MOUNT_PATH, 0), 0);
    assert_eq!(rmdir(MOUNT_PATH), 0);

    assert_eq!(unlink(GLOBAL_PATH), 0);
    assert_eq!(unlink(PATH), 0);
    println!("FS_PHASE3_PROBE_PASS");
}

fn persist_prepare() {
    cleanup();
    let fd = open(TARGET, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0);
    let fd = fd as usize;
    assert_eq!(write(fd, b"phase3-persistent-data"), 22);
    assert_eq!(syncfs(fd), 0);
    assert_eq!(close(fd), 0);
    println!("FS_WRITEBACK_PERSIST_PREPARE_PASS");
}

fn persist_verify() {
    let fd = open(TARGET, O_RDONLY, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    let mut data = [0u8; 22];
    assert_eq!(pread(fd, &mut data, 0), 22);
    assert_eq!(&data, b"phase3-persistent-data");
    assert_eq!(close(fd), 0);
    assert_eq!(unlink(TARGET), 0);
    println!("FS_WRITEBACK_PERSIST_VERIFY_PASS");
}

/// Requires a kernel built with `debug_traces`. The feature exposes a
/// one-shot PageCache fault through `/proc/respos_perf`; release kernels do
/// not accept the control command.
fn fault() {
    cleanup();
    let writer = open(TARGET, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(writer >= 0);
    let writer = writer as usize;
    let writer_dup = dup(writer);
    assert!(writer_dup >= 0);
    let writer_dup = writer_dup as usize;
    let observer = open(TARGET, O_RDONLY, 0);
    assert!(observer >= 0);
    let observer = observer as usize;
    let payload = b"dirty-before-injected-error";
    assert_eq!(write(writer, payload), payload.len() as isize);

    let control = open(FAULT_CONTROL, O_WRONLY, 0);
    assert!(control >= 0);
    let control = control as usize;
    let command = b"fail_writeback";
    assert_eq!(write(control, command), command.len() as isize);
    assert_eq!(close(control), 0);

    assert_eq!(fsync(observer), EIO);
    assert_eq!(fsync(observer), 0);

    // A new description samples the current sequence and must not inherit an
    // error that happened before open. The older writer still observes it.
    let newcomer = open(TARGET, O_RDONLY, 0);
    assert!(newcomer >= 0);
    let newcomer = newcomer as usize;
    assert_eq!(fsync(newcomer), 0);
    assert_eq!(fsync(writer), EIO);
    assert_eq!(fsync(writer_dup), 0);
    assert_eq!(fsync(writer), 0);

    assert_eq!(close(newcomer), 0);
    assert_eq!(close(observer), 0);
    assert_eq!(close(writer_dup), 0);
    assert_eq!(close(writer), 0);
    assert_eq!(unlink(TARGET), 0);
    println!("FS_WRITEBACK_FAULT_PASS");
}

#[unsafe(no_mangle)]
fn main(argc: usize, argv: &[&str]) -> i32 {
    match if argc > 1 { argv[1] } else { "normal" } {
        "normal" => normal(),
        "phase3" => phase3(),
        "fault" => fault(),
        "persist-prepare" => persist_prepare(),
        "persist-verify" => persist_verify(),
        _ => {
            panic!("usage: fs_writeback_probe [normal|phase3|fault|persist-prepare|persist-verify]")
        }
    }
    0
}
