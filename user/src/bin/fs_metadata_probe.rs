#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_DIRECTORY, O_RDONLY, O_RDWR, O_TRUNC, Stat, TimeSpec, chmod, chown, close, fchmod,
    fchown, fstat, fsync, futimens, getgid, getuid, link, mkdir, open, rmdir, stat, unlink,
    utimens, write,
};

const FILE_PATH: &str = "/respos-fsmeta-file\0";
const LINK_PATH: &str = "/respos-fsmeta-link\0";
const UNLINKED_PATH: &str = "/respos-fsmeta-unlinked\0";
const DIR_PATH: &str = "/respos-fsmeta-dir\0";
const MODE_MASK: u32 = 0o7777;

fn mode(stat: &Stat) -> u32 {
    stat.st_mode & MODE_MASK
}

fn path_stat(path: &str) -> Stat {
    let mut value = Stat::default();
    assert_eq!(stat(path, &mut value), 0, "stat failed: {}", path);
    value
}

fn try_path_stat(path: &str) -> Result<Stat, isize> {
    let mut value = Stat::default();
    let ret = stat(path, &mut value);
    if ret == 0 { Ok(value) } else { Err(ret) }
}

fn fd_stat(fd: usize) -> Stat {
    let mut value = Stat::default();
    assert_eq!(fstat(fd, &mut value), 0, "fstat failed: {}", fd);
    value
}

fn cleanup() {
    let _ = unlink(LINK_PATH);
    let _ = unlink(FILE_PATH);
    let _ = unlink(UNLINKED_PATH);
    let _ = rmdir(DIR_PATH);
}

fn run_normal() {
    cleanup();

    let fd = open(FILE_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o640);
    assert!(fd >= 0, "create failed: {}", fd);
    let fd = fd as usize;
    assert_eq!(write(fd, b"metadata"), 8);
    assert_eq!(link(FILE_PATH, LINK_PATH), 0);

    let original = try_path_stat(FILE_PATH);
    let alias = try_path_stat(LINK_PATH);
    let metadata_path = match (&original, &alias) {
        (Ok(original), Ok(alias))
            if original.st_ino == alias.st_ino && original.st_nlink == 2 && alias.st_nlink == 2 =>
        {
            Some(LINK_PATH)
        }
        (Ok(original), Ok(alias)) => {
            println!(
                "FS_METADATA_EXPECTED_FAIL name=hardlink_identity source_ino={} alias_ino={} source_nlink={} alias_nlink={}",
                original.st_ino, alias.st_ino, original.st_nlink, alias.st_nlink
            );
            Some(LINK_PATH)
        }
        (Ok(_), Err(alias_ret)) => {
            println!(
                "FS_METADATA_EXPECTED_FAIL name=hardlink_alias_visibility source_ret=0 alias_ret={}",
                alias_ret
            );
            Some(FILE_PATH)
        }
        (Err(source_ret), Ok(_)) => {
            println!(
                "FS_METADATA_EXPECTED_FAIL name=hardlink_source_survival source_ret={} alias_ret=0",
                source_ret
            );
            Some(LINK_PATH)
        }
        (Err(source_ret), Err(alias_ret)) => {
            println!(
                "FS_METADATA_EXPECTED_FAIL name=hardlink_path_visibility source_ret={} alias_ret={}",
                source_ret, alias_ret
            );
            None
        }
    };

    let uid = getuid();
    let gid = getgid();
    assert!(uid >= 0 && gid >= 0);
    if let Some(metadata_path) = metadata_path {
        assert_eq!(chmod(metadata_path, 0o601), 0);
        assert_eq!(mode(&path_stat(metadata_path)), 0o601, "mode update");
        assert_eq!(chown(metadata_path, uid as usize, gid as usize), 0);
        let value = path_stat(metadata_path);
        assert_eq!((value.st_uid, value.st_gid), (uid as u32, gid as u32));

        let times = [TimeSpec { sec: 1, nsec: 0 }, TimeSpec { sec: 2, nsec: 0 }];
        assert_eq!(utimens(metadata_path, &times), 0);
        let value = path_stat(metadata_path);
        assert_eq!(value.st_atime.sec, 1, "atime update");
        assert_eq!(value.st_mtime.sec, 2, "mtime update");
    }

    assert_eq!(fsync(fd), 0);
    assert_eq!(close(fd), 0);
    if let Some(metadata_path) = metadata_path {
        let reopened = open(metadata_path, O_RDONLY, 0);
        assert!(reopened >= 0, "reopen failed: {}", reopened);
        let reopened = reopened as usize;
        let after_reopen = fd_stat(reopened);
        assert_eq!(mode(&after_reopen), 0o601);
        assert_eq!(
            (after_reopen.st_uid, after_reopen.st_gid),
            (uid as u32, gid as u32)
        );
        assert_eq!(close(reopened), 0);
    }

    let unlinked = open(UNLINKED_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o640);
    assert!(unlinked >= 0, "create unlinked file failed: {}", unlinked);
    let unlinked = unlinked as usize;
    let before = fd_stat(unlinked);
    assert_eq!(unlink(UNLINKED_PATH), 0);
    let chmod_ret = fchmod(unlinked, 0o600);
    let chown_ret = fchown(unlinked, uid as usize, gid as usize);
    let unlinked_times = [TimeSpec { sec: 3, nsec: 0 }, TimeSpec { sec: 4, nsec: 0 }];
    let futimens_ret = futimens(unlinked, &unlinked_times);
    let after = fd_stat(unlinked);
    if chmod_ret == 0
        && chown_ret == 0
        && futimens_ret == 0
        && mode(&after) == 0o600
        && (after.st_uid, after.st_gid) == (uid as u32, gid as u32)
        && after.st_atime.sec == 3
        && after.st_mtime.sec == 4
    {
        println!("FS_METADATA_UNLINKED_FD_ATTRIBUTES_PASS");
    } else {
        println!(
            "FS_METADATA_EXPECTED_FAIL name=unlinked_fd_attributes chmod_ret={} chown_ret={} futimens_ret={} before={:o} after={:o}",
            chmod_ret,
            chown_ret,
            futimens_ret,
            mode(&before),
            mode(&after)
        );
    }
    assert_eq!(close(unlinked), 0);

    cleanup();
    println!("FS_METADATA_PROBE_PASS");
}

fn prepare_directory_persistence() {
    let _ = rmdir(DIR_PATH);
    assert_eq!(mkdir(DIR_PATH, 0o755), 0);
    assert_eq!(chmod(DIR_PATH, 0o711), 0);
    match try_path_stat(DIR_PATH) {
        Ok(value) if mode(&value) == 0o711 => {}
        Ok(value) => println!(
            "FS_METADATA_EXPECTED_FAIL name=directory_chmod_visibility expected=711 actual={:o}",
            mode(&value)
        ),
        Err(ret) => println!(
            "FS_METADATA_EXPECTED_FAIL name=directory_chmod_visibility stat_ret={}",
            ret
        ),
    }
    let fd = open(DIR_PATH, O_RDONLY | O_DIRECTORY, 0);
    if fd >= 0 {
        let fd = fd as usize;
        let value = fd_stat(fd);
        assert_eq!(mode(&value), 0o711, "directory fd mode after chmod");
        println!("FS_METADATA_DIRECTORY_FD_MODE_PASS mode=711");
        assert_eq!(fsync(fd), 0);
        assert_eq!(close(fd), 0);
    } else {
        println!(
            "FS_METADATA_EXPECTED_FAIL name=directory_reopen_after_chmod ret={}",
            fd
        );
    }
    println!("FS_METADATA_PREPARE_PASS mode=711");
}

fn verify_directory_persistence() {
    match try_path_stat(DIR_PATH) {
        Ok(value) if mode(&value) == 0o711 => {
            println!("FS_METADATA_DIRECTORY_PERSISTENCE_PASS mode=711");
        }
        Ok(value) => println!(
            "FS_METADATA_EXPECTED_FAIL name=directory_chmod_persistence expected=711 actual={:o}",
            mode(&value)
        ),
        Err(ret) => println!(
            "FS_METADATA_EXPECTED_FAIL name=directory_chmod_persistence stat_ret={}",
            ret
        ),
    }
    let _ = rmdir(DIR_PATH);
    println!("FS_METADATA_VERIFY_PASS");
}

#[unsafe(no_mangle)]
fn main(argc: usize, argv: &[&str]) -> i32 {
    let mode = if argc > 1 { argv[1] } else { "normal" };
    match mode {
        "normal" => run_normal(),
        "prepare" => prepare_directory_persistence(),
        "verify" => verify_directory_persistence(),
        "cleanup" => cleanup(),
        _ => {
            println!("usage: fs_metadata_probe [normal|prepare|verify|cleanup]");
            return 2;
        }
    }
    0
}
