#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    AT_EMPTY_PATH, AT_FDCWD, AT_SYMLINK_FOLLOW, AT_SYMLINK_NOFOLLOW, O_APPEND, O_CLOEXEC, O_CREATE,
    O_NOATIME, O_NOFOLLOW, O_NONBLOCK, O_PATH, O_RDONLY, O_RDWR, O_TMPFILE, O_TRUNC, O_WRONLY,
    Stat, chmod, chown, close, dup, faccessat2, fchmod, fcntl, fdatasync, fstat, fstatat, fsync,
    getdents64, link, linkat, mkdir, mount, open, read, rename, rmdir, setfsgid, setfsuid,
    setgroups, setxattr, symlink, umask, umount2, unlink, write,
};

const ROOT: &str = "/phase4-probe\0";
const TARGET: &str = "/phase4-probe/target\0";
const TARGET_SLASH: &str = "/phase4-probe/target/\0";
const LINK: &str = "/phase4-probe/link\0";
const LINK_SLASH: &str = "/phase4-probe/link/\0";
const HARD_LINK: &str = "/phase4-probe/hard-link\0";
const FOLLOW_LINK: &str = "/phase4-probe/follow-link\0";
const RENAMED_LINK: &str = "/phase4-probe/renamed-link\0";
const MISSING_SLASH: &str = "/phase4-probe/missing/\0";
const MODE_FILE: &str = "/phase4-probe/mode-file\0";
const TMP_LINK: &str = "/phase4-probe/tmp-link\0";
const GROUP_DIR: &str = "/phase4-probe/group-dir\0";
const GROUP_FILE: &str = "/phase4-probe/group-dir/member\0";
const NONGROUP_FILE: &str = "/phase4-probe/group-dir/nonmember\0";
const STICKY_DIR: &str = "/phase4-probe/sticky\0";
const STICKY_FILE: &str = "/phase4-probe/sticky/owned\0";
const BIND_SOURCE: &str = "/phase4-probe/bind-source\0";
const BIND_FILE: &str = "/phase4-probe/bind-source/file\0";
const BIND_TARGET: &str = "/phase4-probe/bind-target\0";
const BIND_TARGET_FILE: &str = "/phase4-probe/bind-target/file\0";
const EMPTY: &str = "\0";

const F_GETFD: usize = 1;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const FD_CLOEXEC: usize = 1;
const R_OK: usize = 4;
const W_OK: usize = 2;
const MS_RDONLY: usize = 1;
const MS_BIND: usize = 4096;

fn create(path: &str, mode: usize) -> usize {
    let fd = open(path, O_CREATE | O_TRUNC | O_RDWR, mode);
    assert!(fd >= 0, "create {}: {}", path, fd);
    fd as usize
}

fn stat_at(path: &str, flags: usize) -> Stat {
    let mut stat = Stat::default();
    assert_eq!(fstatat(AT_FDCWD, path, &mut stat, flags), 0);
    stat
}

fn cleanup() {
    let _ = umount2(BIND_TARGET, 0);
    for path in [
        BIND_TARGET_FILE,
        BIND_FILE,
        STICKY_FILE,
        GROUP_FILE,
        NONGROUP_FILE,
        RENAMED_LINK,
        FOLLOW_LINK,
        HARD_LINK,
        LINK,
        TARGET,
        MODE_FILE,
        TMP_LINK,
    ] {
        let _ = unlink(path);
    }
    for path in [BIND_TARGET, BIND_SOURCE, STICKY_DIR, GROUP_DIR, ROOT] {
        let _ = rmdir(path);
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    cleanup();
    assert_eq!(mkdir(ROOT, 0o755), 0);

    let target_fd = create(TARGET, 0o644);
    assert_eq!(write(target_fd, b"target"), 6);
    assert_eq!(close(target_fd), 0);
    assert_eq!(symlink("target\0", LINK), 0);

    let target = stat_at(TARGET, 0);
    let link_stat = stat_at(LINK, AT_SYMLINK_NOFOLLOW);
    assert_ne!(target.st_ino, link_stat.st_ino);
    let symlink_fd = open(LINK, O_PATH | O_NOFOLLOW, 0);
    assert!(symlink_fd >= 0);
    let mut symlink_fd_stat = Stat::default();
    assert_eq!(fstat(symlink_fd as usize, &mut symlink_fd_stat), 0);
    assert_eq!(symlink_fd_stat.st_ino, link_stat.st_ino);
    assert_eq!(close(symlink_fd as usize), 0);
    assert_eq!(open(LINK_SLASH, O_PATH | O_NOFOLLOW, 0), -20);
    assert_eq!(link(LINK, HARD_LINK), 0);
    let hard_link = stat_at(HARD_LINK, AT_SYMLINK_NOFOLLOW);
    assert_eq!(hard_link.st_ino, link_stat.st_ino);
    assert_eq!(
        linkat(AT_FDCWD, LINK, AT_FDCWD, FOLLOW_LINK, AT_SYMLINK_FOLLOW),
        0
    );
    assert_eq!(stat_at(FOLLOW_LINK, 0).st_ino, target.st_ino);
    assert_eq!(rename(LINK, RENAMED_LINK), 0);
    assert_eq!(
        stat_at(RENAMED_LINK, AT_SYMLINK_NOFOLLOW).st_ino,
        link_stat.st_ino
    );
    assert_eq!(stat_at(TARGET, 0).st_ino, target.st_ino);

    assert_eq!(open(TARGET_SLASH, O_RDONLY, 0), -20);
    assert_eq!(open(MISSING_SLASH, O_CREATE | O_RDWR, 0o600), -21);
    assert_eq!(unlink(TARGET_SLASH), -20);
    assert_eq!(rename("/phase4-probe/renamed-link/\0", LINK), -20);
    assert_eq!(rename(TARGET, MISSING_SLASH), -20);
    assert_eq!(link(TARGET, MISSING_SLASH), -2);

    assert_eq!(chmod(TARGET, 0), 0);
    let path_fd = open(TARGET, O_PATH | O_CLOEXEC, 0);
    assert!(path_fd >= 0);
    let path_fd = path_fd as usize;
    assert_eq!(fcntl(path_fd, F_GETFD, 0), FD_CLOEXEC as isize);
    assert_eq!(fcntl(path_fd, F_GETFL, 0), O_PATH as isize);
    let mut byte = [0u8; 1];
    assert_eq!(read(path_fd, &mut byte), -9);
    assert_eq!(fchmod(path_fd, 0o600), -9);
    assert_eq!(fcntl(path_fd, F_SETFL, O_NONBLOCK), -9);
    assert_eq!(fsync(path_fd), -9);
    assert_eq!(fdatasync(path_fd), -9);
    let mut dirents = [0u8; 64];
    assert_eq!(getdents64(path_fd, &mut dirents), -9);
    let mut path_stat = Stat::default();
    assert_eq!(fstat(path_fd, &mut path_stat), 0);
    let mut empty_stat = Stat::default();
    assert_eq!(
        fstatat(path_fd as isize, EMPTY, &mut empty_stat, AT_EMPTY_PATH),
        0
    );
    assert_eq!(empty_stat.st_ino, path_stat.st_ino);
    assert_eq!(close(path_fd), 0);
    assert_eq!(chmod(TARGET, 0o640), 0);

    let tmp_fd = open(ROOT, O_TMPFILE | O_RDWR, 0o600);
    assert!(tmp_fd >= 0);
    let tmp_fd = tmp_fd as usize;
    let mut tmp_stat = Stat::default();
    assert_eq!(fstat(tmp_fd, &mut tmp_stat), 0);
    assert_eq!(tmp_stat.st_nlink, 0);
    assert_eq!(write(tmp_fd, b"tmpfile"), 7);
    assert_eq!(
        linkat(tmp_fd as isize, EMPTY, AT_FDCWD, TMP_LINK, AT_EMPTY_PATH),
        0
    );
    let linked_tmp = open(TMP_LINK, O_RDONLY, 0);
    assert!(linked_tmp >= 0);
    let mut tmp_data = [0u8; 7];
    assert_eq!(read(linked_tmp as usize, &mut tmp_data), 7);
    assert_eq!(&tmp_data, b"tmpfile");
    assert_eq!(close(linked_tmp as usize), 0);
    assert_eq!(close(tmp_fd), 0);

    let fd = open(TARGET, O_RDWR | O_APPEND | O_CLOEXEC, 0);
    assert!(fd >= 0);
    let fd = fd as usize;
    let duplicate = dup(fd);
    assert!(duplicate >= 0);
    let duplicate = duplicate as usize;
    assert_eq!(fcntl(fd, F_GETFD, 0), FD_CLOEXEC as isize);
    assert_eq!(fcntl(duplicate, F_GETFD, 0), 0);
    assert_eq!(fcntl(duplicate, F_SETFL, O_NONBLOCK), 0);
    let shared_flags = fcntl(fd, F_GETFL, 0) as usize;
    assert_ne!(shared_flags & O_NONBLOCK, 0);
    assert_eq!(shared_flags & O_APPEND, 0);
    assert_eq!(close(duplicate), 0);
    assert_eq!(close(fd), 0);

    let previous_umask = umask(0o027);
    assert!(previous_umask >= 0);
    let mode_fd = create(MODE_FILE, 0o666);
    assert_eq!(close(mode_fd), 0);
    assert_eq!(stat_at(MODE_FILE, 0).st_mode & 0o777, 0o640);
    assert_eq!(umask(previous_umask as usize), 0o027);

    assert_eq!(chown(TARGET, 1234, 3333), 0);
    assert_eq!(chmod(TARGET, 0o640), 0);
    assert_eq!(setgroups(&[3333]), 0);
    assert_eq!(setfsuid(2000), 0);
    let group_fd = open(TARGET, O_RDONLY, 0);
    assert!(group_fd >= 0);
    assert_eq!(close(group_fd as usize), 0);
    assert_eq!(setgroups(&[]), 0);
    assert_eq!(open(TARGET, O_RDONLY, 0), -13);
    assert_eq!(open(TARGET, O_RDONLY | O_NOATIME, 0), -1);
    // faccessat2 without AT_EACCESS uses the real IDs.  The probe process
    // remains real-uid 0 while fsuid is lowered, so Linux grants this check.
    assert_eq!(faccessat2(AT_FDCWD, TARGET, R_OK | W_OK, 0), 0);
    assert_eq!(setfsuid(0), 2000);

    assert_eq!(mkdir(GROUP_DIR, 0o755), 0);
    assert_eq!(chown(GROUP_DIR, 0, 3333), 0);
    assert_eq!(chmod(GROUP_DIR, 0o2777), 0);
    assert_eq!(setgroups(&[3333]), 0);
    assert_eq!(setfsgid(4444), 0);
    assert_eq!(setfsuid(2000), 0);
    let member = create(GROUP_FILE, 0o2666);
    assert_eq!(close(member), 0);
    let member_stat = stat_at(GROUP_FILE, 0);
    assert_eq!(member_stat.st_gid, 3333);
    assert_ne!(member_stat.st_mode & 0o2000, 0);
    assert_eq!(setgroups(&[]), 0);
    let nonmember = create(NONGROUP_FILE, 0o2666);
    assert_eq!(close(nonmember), 0);
    let nonmember_stat = stat_at(NONGROUP_FILE, 0);
    assert_eq!(nonmember_stat.st_gid, 3333);
    assert_eq!(nonmember_stat.st_mode & 0o2000, 0);
    assert_eq!(setfsuid(0), 2000);
    assert_eq!(setfsgid(0), 4444);

    assert_eq!(mkdir(STICKY_DIR, 0o755), 0);
    assert_eq!(chmod(STICKY_DIR, 0o1777), 0);
    let sticky = create(STICKY_FILE, 0o600);
    assert_eq!(close(sticky), 0);
    assert_eq!(chown(STICKY_FILE, 1001, 1001), 0);
    assert_eq!(setfsuid(1002), 0);
    assert_eq!(unlink(STICKY_FILE), -1);
    assert_eq!(setfsuid(0), 1002);

    assert_eq!(mkdir(BIND_SOURCE, 0o755), 0);
    let bind_fd = create(BIND_FILE, 0o644);
    assert_eq!(close(bind_fd), 0);
    assert_eq!(mkdir(BIND_TARGET, 0o755), 0);
    assert_eq!(
        mount(BIND_SOURCE, BIND_TARGET, "\0", MS_BIND | MS_RDONLY, 0),
        0
    );
    let ro_fd = open(BIND_TARGET_FILE, O_RDONLY, 0);
    assert!(ro_fd >= 0);
    assert_eq!(close(ro_fd as usize), 0);
    assert_eq!(open(BIND_TARGET_FILE, O_WRONLY, 0), -30);
    assert_eq!(faccessat2(AT_FDCWD, BIND_TARGET_FILE, W_OK, 0), -30);
    assert_eq!(chmod(BIND_TARGET_FILE, 0o600), -30);
    assert_eq!(setxattr(BIND_TARGET_FILE, "user.phase4\0", b"x", 0), -30);
    assert_eq!(umount2(BIND_TARGET, 0), 0);

    cleanup();
    println!("FS_PHASE4_PROBE_PASS");
    0
}
