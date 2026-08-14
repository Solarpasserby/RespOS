#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, SEEK_SET, Stat, chdir, close, exit, fork, fstat,
    getdents64, link, lseek, mkdir, open, pipe, read, rename, rmdir, stat, symlink, unlink,
    waitpid, write, yield_,
};

const DIR_A: &str = "/respos-ns-a\0";
const DIR_B: &str = "/respos-ns-b\0";
const SOURCE: &str = "/respos-ns-a/source\0";
const TYPE_LINK: &str = "/respos-ns-a/type-link\0";
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
const CWD_HOLD: &str = "/respos-ns-cwd-hold\0";
const CWD_REUSE: &str = "/respos-ns-cwd-reuse\0";
const DOT: &str = ".\0";
const DIRENT64_HEADER_SIZE: usize = 19;
const DT_DIR: u8 = 0o4;
const DT_REG: u8 = 0o10;
const DT_LNK: u8 = 0o12;

fn assert_dirent_type(dir_path: &str, name: &[u8], expected: u8) {
    let fd = open(dir_path, O_RDONLY, 0);
    assert!(fd >= 0, "open directory failed: {} ret={}", dir_path, fd);
    let fd = fd as usize;
    let mut buf = [0u8; 1024];
    let mut found = false;

    loop {
        let size = getdents64(fd, &mut buf);
        assert!(size >= 0, "getdents64 failed: {} ret={}", dir_path, size);
        if size == 0 {
            break;
        }
        let size = size as usize;
        let mut offset = 0;
        while offset + DIRENT64_HEADER_SIZE <= size {
            let reclen = u16::from_le_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
            assert!(
                reclen >= DIRENT64_HEADER_SIZE && offset + reclen <= size,
                "invalid dirent record"
            );
            let name_start = offset + DIRENT64_HEADER_SIZE;
            let name_len = buf[name_start..offset + reclen]
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(offset + reclen - name_start);
            if &buf[name_start..name_start + name_len] == name {
                assert_eq!(buf[offset + 18], expected, "unexpected d_type");
                found = true;
            }
            offset += reclen;
        }
        assert_eq!(offset, size, "truncated dirent record");
    }
    assert_eq!(close(fd), 0);
    assert!(found, "directory entry not found: {}", dir_path);
}

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
        TYPE_LINK,
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
    let _ = rmdir(CWD_HOLD);
    let _ = rmdir(CWD_REUSE);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    cleanup();
    assert_eq!(mkdir(DIR_A, 0o755), 0);
    assert_eq!(mkdir(DIR_B, 0o755), 0);

    let source_fd = create(SOURCE, b"source");
    assert_eq!(symlink("source\0", TYPE_LINK), 0);
    assert_dirent_type("/\0", b"respos-ns-a", DT_DIR);
    assert_dirent_type(DIR_A, b"source", DT_REG);
    assert_dirent_type(DIR_A, b"type-link", DT_LNK);
    println!("FS_NAMESPACE_DIRENT_TYPE_PASS");
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

    // A cwd is an inode reference even when no File object is open.  rmdir
    // removes the namespace link, but the directory must remain usable via
    // the child's cwd until that final Path/Dentry reference disappears.
    assert_eq!(mkdir(CWD_HOLD, 0o755), 0);
    let cwd_ino = path_stat(CWD_HOLD).st_ino;
    let mut ready = [0i32; 2];
    let mut resume = [0i32; 2];
    assert_eq!(pipe(&mut ready), 0);
    assert_eq!(pipe(&mut resume), 0);
    let cwd_child = fork();
    assert!(cwd_child >= 0);
    if cwd_child == 0 {
        assert_eq!(close(ready[0] as usize), 0);
        assert_eq!(close(resume[1] as usize), 0);
        assert_eq!(chdir(CWD_HOLD), 0);
        assert_eq!(write(ready[1] as usize, b"r"), 1);
        let mut signal = [0u8; 1];
        assert_eq!(read(resume[0] as usize, &mut signal), 1);
        let cwd_fd = open(DOT, O_RDONLY, 0);
        assert!(cwd_fd >= 0);
        let cwd_stat = fd_stat(cwd_fd as usize);
        assert_eq!(cwd_stat.st_ino, cwd_ino);
        assert_eq!(cwd_stat.st_nlink, 0);
        assert_eq!(close(cwd_fd as usize), 0);
        exit(0);
    }
    assert_eq!(close(ready[1] as usize), 0);
    assert_eq!(close(resume[0] as usize), 0);
    let mut signal = [0u8; 1];
    assert_eq!(read(ready[0] as usize, &mut signal), 1);
    assert_eq!(rmdir(CWD_HOLD), 0);
    assert_eq!(mkdir(CWD_REUSE, 0o755), 0);
    assert_ne!(path_stat(CWD_REUSE).st_ino, cwd_ino);
    assert_eq!(write(resume[1] as usize, b"x"), 1);
    let mut cwd_status = 0;
    assert_eq!(waitpid(cwd_child as usize, &mut cwd_status), cwd_child);
    assert_eq!(cwd_status, 0);
    assert_eq!(close(ready[0] as usize), 0);
    assert_eq!(close(resume[1] as usize), 0);
    assert_eq!(rmdir(CWD_REUSE), 0);
    println!("FS_NAMESPACE_CWD_UNLINK_PASS");

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
