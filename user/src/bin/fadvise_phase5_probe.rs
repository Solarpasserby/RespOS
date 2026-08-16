#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, dup, fadvise64, fsync, getrusage_raw, lseek, open, pipe, read, unlink, write, RUsage,
    O_CREATE, O_RDWR, O_TRUNC, SEEK_SET,
};

const PAGE_SIZE: usize = 4096;
const FILE_PAGES: usize = 40;
const PATH: &str = "/tmp/respos_fadvise_phase5.tmp\0";
const RUSAGE_THREAD: isize = 1;
const POSIX_FADV_NORMAL: usize = 0;
const POSIX_FADV_RANDOM: usize = 1;
const POSIX_FADV_SEQUENTIAL: usize = 2;
const POSIX_FADV_WILLNEED: usize = 3;
const POSIX_FADV_DONTNEED: usize = 4;
const POSIX_FADV_NOREUSE: usize = 5;
const EBADF: isize = 9;
const EINVAL: isize = 22;
const ESPIPE: isize = 29;

fn inblock() -> isize {
    let mut usage = RUsage::default();
    assert_eq!(getrusage_raw(RUSAGE_THREAD, &mut usage), 0);
    usage.ru_inblock
}

fn seek_read_byte(fd: usize, offset: usize) -> u8 {
    assert_eq!(lseek(fd, offset as isize, SEEK_SET), offset as isize);
    let mut byte = [0u8; 1];
    assert_eq!(read(fd, &mut byte), 1);
    byte[0]
}

fn fill_file(fd: usize) {
    let mut page = [0u8; PAGE_SIZE];
    for page_idx in 0..FILE_PAGES {
        page.fill((page_idx as u8).wrapping_add(1));
        assert_eq!(write(fd, &page), PAGE_SIZE as isize);
    }
    assert_eq!(fsync(fd), 0);
}

fn test_errors_and_all_advice(fd: usize) {
    for advice in POSIX_FADV_NORMAL..=POSIX_FADV_NOREUSE {
        assert_eq!(fadvise64(fd, 0, 0, advice), 0);
    }
    assert_eq!(fadvise64(fd, -1, 0, POSIX_FADV_NORMAL), -EINVAL);
    assert_eq!(fadvise64(fd, 0, -1, POSIX_FADV_NORMAL), -EINVAL);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_NOREUSE + 1), -EINVAL);
    assert_eq!(fadvise64(usize::MAX / 2, 0, 0, POSIX_FADV_NORMAL), -EBADF);

    let mut pipefd = [-1i32; 2];
    assert_eq!(pipe(&mut pipefd), 0);
    assert_eq!(
        fadvise64(pipefd[0] as usize, 0, 0, POSIX_FADV_NORMAL),
        -ESPIPE
    );
    assert_eq!(close(pipefd[0] as usize), 0);
    assert_eq!(close(pipefd[1] as usize), 0);
    println!("FADVISE_PHASE5 errors/all-advice PASS");
}

fn test_read_ahead_modes(fd: usize) {
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_RANDOM), 0);
    let before_random = inblock();
    assert_eq!(seek_read_byte(fd, 0), 1);
    let after_random_first = inblock();
    assert!(after_random_first > before_random);
    assert_eq!(seek_read_byte(fd, PAGE_SIZE), 2);
    assert!(inblock() > after_random_first);

    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_NORMAL), 0);
    assert_eq!(seek_read_byte(fd, 0), 1);
    let after_normal_first = inblock();
    assert_eq!(seek_read_byte(fd, 8 * PAGE_SIZE), 9);
    assert_eq!(inblock(), after_normal_first);
    assert_eq!(seek_read_byte(fd, 20 * PAGE_SIZE), 21);
    assert!(inblock() > after_normal_first);

    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_SEQUENTIAL), 0);
    assert_eq!(seek_read_byte(fd, 0), 1);
    let after_sequential_first = inblock();
    assert_eq!(seek_read_byte(fd, 20 * PAGE_SIZE), 21);
    assert_eq!(inblock(), after_sequential_first);
    println!("FADVISE_PHASE5 random/normal/sequential PASS");
}

fn test_open_description_state(fd: usize) {
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_NORMAL), 0);
    let duplicate = dup(fd);
    assert!(duplicate >= 0);
    let duplicate = duplicate as usize;
    assert_eq!(fadvise64(duplicate, 0, 0, POSIX_FADV_RANDOM), 0);
    assert_eq!(seek_read_byte(fd, 0), 1);
    let after_duplicate_first = inblock();
    assert_eq!(seek_read_byte(fd, PAGE_SIZE), 2);
    assert!(inblock() > after_duplicate_first);
    assert_eq!(close(duplicate), 0);

    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_RANDOM), 0);
    let independent = open(PATH, O_RDWR, 0);
    assert!(independent >= 0);
    let independent = independent as usize;
    assert_eq!(seek_read_byte(independent, 0), 1);
    let after_independent_first = inblock();
    assert_eq!(seek_read_byte(independent, 8 * PAGE_SIZE), 9);
    assert_eq!(inblock(), after_independent_first);
    assert_eq!(close(independent), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_NORMAL), 0);
    println!("FADVISE_PHASE5 open-description sharing/isolation PASS");
}

fn test_willneed_and_dontneed_ranges(fd: usize) {
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_RANDOM), 0);
    let before_willneed = inblock();
    assert_eq!(
        fadvise64(fd, (10 * PAGE_SIZE) as isize, 1, POSIX_FADV_WILLNEED),
        0
    );
    let after_willneed = inblock();
    assert!(after_willneed > before_willneed);
    assert_eq!(seek_read_byte(fd, 10 * PAGE_SIZE), 11);
    assert_eq!(inblock(), after_willneed);

    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(seek_read_byte(fd, 0), 1);
    let cached = inblock();
    assert_eq!(
        fadvise64(fd, 1, (PAGE_SIZE - 2) as isize, POSIX_FADV_DONTNEED),
        0
    );
    assert_eq!(seek_read_byte(fd, 0), 1);
    assert_eq!(inblock(), cached);

    assert_eq!(fadvise64(fd, 0, PAGE_SIZE as isize, POSIX_FADV_DONTNEED), 0);
    let before_full_page_refault = inblock();
    assert_eq!(seek_read_byte(fd, 0), 1);
    assert!(inblock() > before_full_page_refault);
    println!("FADVISE_PHASE5 willneed/full-page-dontneed PASS");
}

fn test_dirty_dontneed_writeback(fd: usize) {
    let target = 35 * PAGE_SIZE;
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_DONTNEED), 0);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_RANDOM), 0);
    assert_eq!(lseek(fd, target as isize, SEEK_SET), target as isize);
    let dirty = [0xa5u8; PAGE_SIZE];
    assert_eq!(write(fd, &dirty), PAGE_SIZE as isize);
    assert_eq!(
        fadvise64(fd, target as isize, PAGE_SIZE as isize, POSIX_FADV_DONTNEED),
        0
    );
    let before_refault = inblock();
    assert_eq!(seek_read_byte(fd, target), 0xa5);
    assert!(inblock() > before_refault);

    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_NOREUSE), 0);
    assert_eq!(seek_read_byte(fd, target), 0xa5);
    assert_eq!(fadvise64(fd, 0, 0, POSIX_FADV_NORMAL), 0);
    println!("FADVISE_PHASE5 dirty-writeback/noreuse-reset PASS");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(PATH);
    let fd = open(PATH, O_CREATE | O_TRUNC | O_RDWR, 0o600);
    assert!(fd >= 0);
    let fd = fd as usize;
    fill_file(fd);
    test_errors_and_all_advice(fd);
    test_read_ahead_modes(fd);
    test_open_description_state(fd);
    test_willneed_and_dontneed_ranges(fd);
    test_dirty_dontneed_writeback(fd);
    assert_eq!(close(fd), 0);
    assert_eq!(unlink(PATH), 0);
    println!("FADVISE_PHASE5 ALL PASS");
    0
}
