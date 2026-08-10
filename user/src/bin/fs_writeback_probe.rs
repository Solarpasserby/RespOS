#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, close, dup, fdatasync, fsync, open, pipe, pread,
    unlink, write,
};

const TARGET: &str = "/respos-writeback-target\0";
const FAULT_CONTROL: &str = "/proc/respos_perf\0";
const EIO: isize = -5;
const EINVAL: isize = -22;

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
    assert_eq!(close(pipe_fds[0] as usize), 0);
    assert_eq!(close(pipe_fds[1] as usize), 0);
    assert_eq!(close(observer), 0);
    assert_eq!(close(writer), 0);
    assert_eq!(unlink(TARGET), 0);
    println!("FS_WRITEBACK_PROBE_PASS");
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
        "fault" => fault(),
        _ => panic!("usage: fs_writeback_probe [normal|fault]"),
    }
    0
}
