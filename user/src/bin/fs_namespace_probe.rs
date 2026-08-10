#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, SEEK_SET, Stat, close, exit, fork, fstat, link, lseek,
    mkdir, open, read, rename, rmdir, stat, unlink, waitpid, write, yield_,
};

const DIR_A: &str = "/respos-ns-a\0";
const DIR_B: &str = "/respos-ns-b\0";
const SOURCE: &str = "/respos-ns-a/source\0";
const ALIAS: &str = "/respos-ns-a/alias\0";
const MOVED: &str = "/respos-ns-b/moved\0";
const REPLACE_SRC: &str = "/respos-ns-a/replace-src\0";
const REPLACE_DST: &str = "/respos-ns-a/replace-dst\0";
const SUB_OLD: &str = "/respos-ns-a/sub\0";
const SUB_NEW: &str = "/respos-ns-b/sub\0";
const CHILD_OLD: &str = "/respos-ns-a/sub/child\0";
const CHILD_NEW: &str = "/respos-ns-b/sub/child\0";
const RACE_A: &str = "/respos-ns-a/race-a\0";
const RACE_B: &str = "/respos-ns-b/race-b\0";
const DIR_REPLACE_SRC: &str = "/respos-ns-a/dir-replace-src\0";
const DIR_REPLACE_DST: &str = "/respos-ns-a/dir-replace-dst\0";

fn path_stat(path: &str) -> Stat {
    let mut value = Stat::default();
    assert_eq!(stat(path, &mut value), 0, "stat failed: {}", path);
    value
}

fn fd_stat(fd: usize) -> Stat {
    let mut value = Stat::default();
    assert_eq!(fstat(fd, &mut value), 0, "fstat failed: {}", fd);
    value
}

fn create(path: &str, data: &[u8]) -> usize {
    let fd = open(path, O_CREATE | O_TRUNC | O_RDWR, 0o640);
    assert!(fd >= 0, "create failed: {} ret={}", path, fd);
    let fd = fd as usize;
    assert_eq!(write(fd, data), data.len() as isize);
    fd
}

fn cleanup() {
    for path in [
        CHILD_OLD,
        CHILD_NEW,
        SOURCE,
        ALIAS,
        MOVED,
        REPLACE_SRC,
        REPLACE_DST,
        RACE_A,
        RACE_B,
    ] {
        let _ = unlink(path);
    }
    let _ = rmdir(SUB_OLD);
    let _ = rmdir(SUB_NEW);
    let _ = rmdir(DIR_REPLACE_SRC);
    let _ = rmdir(DIR_REPLACE_DST);
    let _ = rmdir(DIR_A);
    let _ = rmdir(DIR_B);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    cleanup();
    assert_eq!(mkdir(DIR_A, 0o755), 0);
    assert_eq!(mkdir(DIR_B, 0o755), 0);

    let source_fd = create(SOURCE, b"source");
    let source_ino = fd_stat(source_fd).st_ino;
    assert_eq!(close(source_fd), 0);
    let reopened = open(SOURCE, O_RDONLY, 0);
    assert!(reopened >= 0);
    assert_eq!(fd_stat(reopened as usize).st_ino, source_ino);
    assert_eq!(close(reopened as usize), 0);

    assert_eq!(link(SOURCE, ALIAS), 0);
    assert_eq!(path_stat(SOURCE).st_ino, source_ino);
    assert_eq!(path_stat(ALIAS).st_ino, source_ino);
    assert_eq!(path_stat(ALIAS).st_nlink, 2);
    assert_eq!(rename(SOURCE, MOVED), 0);
    let mut missing = Stat::default();
    assert_eq!(stat(SOURCE, &mut missing), -2);
    assert_eq!(path_stat(MOVED).st_ino, source_ino);
    assert_eq!(path_stat(ALIAS).st_ino, source_ino);

    let moved_fd = open(MOVED, O_RDWR, 0);
    assert!(moved_fd >= 0);
    let moved_fd = moved_fd as usize;
    println!("FS_NAMESPACE_STEP moved_open");
    assert_eq!(unlink(MOVED), 0);
    assert_eq!(fd_stat(moved_fd).st_nlink, 1);
    println!("FS_NAMESPACE_STEP first_unlink");
    assert_eq!(unlink(ALIAS), 0);
    assert_eq!(fd_stat(moved_fd).st_nlink, 0);
    println!("FS_NAMESPACE_STEP last_unlink");
    assert_eq!(lseek(moved_fd, 0, SEEK_SET), 0);
    let mut source_data = [0u8; 6];
    assert_eq!(read(moved_fd, &mut source_data), 6);
    assert_eq!(&source_data, b"source");
    assert_eq!(close(moved_fd), 0);

    let replace_src = create(REPLACE_SRC, b"new");
    let replace_src_ino = fd_stat(replace_src).st_ino;
    assert_eq!(close(replace_src), 0);
    let replace_dst = create(REPLACE_DST, b"old");
    let replace_dst_ino = fd_stat(replace_dst).st_ino;
    assert_ne!(replace_src_ino, replace_dst_ino);
    assert_eq!(rename(REPLACE_SRC, REPLACE_DST), 0);
    println!("FS_NAMESPACE_STEP replace_rename");
    assert_eq!(path_stat(REPLACE_DST).st_ino, replace_src_ino);
    assert_eq!(fd_stat(replace_dst).st_ino, replace_dst_ino);
    assert_eq!(fd_stat(replace_dst).st_nlink, 0);
    assert_eq!(lseek(replace_dst, 0, SEEK_SET), 0);
    let mut old_data = [0u8; 3];
    assert_eq!(read(replace_dst, &mut old_data), 3);
    assert_eq!(&old_data, b"old");
    assert_eq!(close(replace_dst), 0);

    assert_eq!(mkdir(DIR_REPLACE_SRC, 0o755), 0);
    assert_eq!(mkdir(DIR_REPLACE_DST, 0o755), 0);
    let replaced_dir = open(DIR_REPLACE_DST, O_RDONLY, 0);
    assert!(replaced_dir >= 0);
    let replaced_dir = replaced_dir as usize;
    let replaced_dir_ino = fd_stat(replaced_dir).st_ino;
    assert_eq!(rename(DIR_REPLACE_SRC, DIR_REPLACE_DST), 0);
    assert_ne!(path_stat(DIR_REPLACE_DST).st_ino, replaced_dir_ino);
    assert_eq!(fd_stat(replaced_dir).st_ino, replaced_dir_ino);
    assert_eq!(fd_stat(replaced_dir).st_nlink, 0);
    assert_eq!(close(replaced_dir), 0);

    let a_before = path_stat(DIR_A).st_nlink;
    let b_before = path_stat(DIR_B).st_nlink;
    assert_eq!(mkdir(SUB_OLD, 0o755), 0);
    let child = create(CHILD_OLD, b"child");
    let child_ino = fd_stat(child).st_ino;
    assert_eq!(path_stat(DIR_A).st_nlink, a_before + 1);
    assert_eq!(rename(SUB_OLD, SUB_NEW), 0);
    assert_eq!(path_stat(DIR_A).st_nlink, a_before);
    assert_eq!(path_stat(DIR_B).st_nlink, b_before + 1);
    assert_eq!(path_stat(CHILD_NEW).st_ino, child_ino);
    assert_eq!(fd_stat(child).st_ino, child_ino);
    assert_eq!(close(child), 0);

    let race = create(RACE_A, b"race");
    let race_ino = fd_stat(race).st_ino;
    assert_eq!(close(race), 0);
    let child_pid = fork();
    assert!(child_pid >= 0);
    if child_pid == 0 {
        for _ in 0..200 {
            assert_eq!(rename(RACE_A, RACE_B), 0);
            assert_eq!(rename(RACE_B, RACE_A), 0);
        }
        exit(0);
    }
    let mut observations = 0usize;
    for _ in 0..1000 {
        for path in [RACE_A, RACE_B] {
            let fd = open(path, O_RDONLY, 0);
            if fd >= 0 {
                let fd = fd as usize;
                assert_eq!(fd_stat(fd).st_ino, race_ino);
                assert_eq!(close(fd), 0);
                observations += 1;
            }
        }
        let _ = yield_();
    }
    let mut status = 0;
    assert_eq!(waitpid(child_pid as usize, &mut status), child_pid);
    assert_eq!(status, 0);
    assert!(observations > 0);

    cleanup();
    println!("FS_NAMESPACE_PROBE_PASS race_observations={}", observations);
    0
}
