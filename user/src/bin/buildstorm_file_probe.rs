#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    O_CREATE, O_RDONLY, O_RDWR, O_TRUNC, close, fstat, ftruncate, link, mmap, munmap, open, pread,
    pwrite, unlink, write,
};

const OLD_PATH: &str = "/respos-buildstorm-file.tmp\0";
const NEW_PATH: &str = "/respos-buildstorm-file.bin\0";
const MAP_PATH: &str = "/respos-buildstorm-mmap.bin\0";
const REUSE_PATH: &str = "/respos-buildstorm-reuse.bin\0";
const PREFIX_END: usize = 0x2000;
const SECTION_OFFSET: usize = 0x40c370;
const SECTION_LEN: usize = 0x9c0;
const FINAL_SIZE: usize = SECTION_OFFSET + SECTION_LEN;
const MAP_SIZE: usize = FINAL_SIZE.next_multiple_of(4096);

fn pattern(offset: usize) -> u8 {
    ((offset.wrapping_mul(13) ^ (offset >> 12).wrapping_mul(17)) & 0xff) as u8
}

fn fill_pattern(buf: &mut [u8], offset: usize) {
    for (index, byte) in buf.iter_mut().enumerate() {
        *byte = pattern(offset + index);
    }
}

fn verify_pattern(fd: usize, offset: usize, len: usize, buf: &mut [u8]) {
    assert!(len <= buf.len());
    let n = pread(fd, &mut buf[..len], offset as isize);
    assert_eq!(n, len as isize, "pread pattern offset={:#x}", offset);
    for (index, &byte) in buf[..len].iter().enumerate() {
        assert_eq!(
            byte,
            pattern(offset + index),
            "pattern offset={:#x}",
            offset + index
        );
    }
}

fn verify_zero(fd: usize, offset: usize, len: usize, buf: &mut [u8]) {
    assert!(len <= buf.len());
    let n = pread(fd, &mut buf[..len], offset as isize);
    assert_eq!(n, len as isize, "pread zero offset={:#x}", offset);
    assert!(
        buf[..len].iter().all(|&byte| byte == 0),
        "sparse hole contains data at offset={:#x}",
        offset
    );
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let _ = unlink(OLD_PATH);
    let _ = unlink(NEW_PATH);
    let _ = unlink(MAP_PATH);
    let _ = unlink(REUSE_PATH);

    let mut sections = [0u8; SECTION_LEN];
    for (index, byte) in sections.iter_mut().enumerate() {
        *byte = 0xa5 ^ (index as u8).wrapping_mul(29);
    }

    // ld.bfd can mmap its output while it is still short, then grow the file
    // and fill the newly valid tail through that existing MAP_SHARED mapping.
    // The VMA must not keep the mmap-time EOF as its permanent writeback cap.
    let map_fd = open(MAP_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o755);
    assert!(map_fd >= 0, "mmap create failed: {}", map_fd);
    let map_fd = map_fd as usize;
    let mapped = mmap(0, MAP_SIZE, 0x3, 0x1, map_fd as isize, 0);
    assert!(mapped >= 0, "mmap failed: {}", mapped);
    assert_eq!(ftruncate(map_fd, FINAL_SIZE), 0);
    unsafe {
        core::slice::from_raw_parts_mut(mapped as *mut u8, MAP_SIZE)[SECTION_OFFSET..FINAL_SIZE]
            .copy_from_slice(&sections);
    }
    assert_eq!(munmap(mapped as usize, MAP_SIZE), 0);
    assert_eq!(close(map_fd), 0);

    let map_fd = open(MAP_PATH, O_RDONLY, 0);
    assert!(map_fd >= 0, "mmap reopen failed: {}", map_fd);
    let map_fd = map_fd as usize;
    let mut actual_sections = [0u8; SECTION_LEN];
    assert_eq!(
        pread(map_fd, &mut actual_sections, SECTION_OFFSET as isize),
        SECTION_LEN as isize
    );
    assert_eq!(actual_sections, sections, "mmap tail mismatch");
    assert_eq!(close(map_fd), 0);
    assert_eq!(unlink(MAP_PATH), 0);

    // lld reuses an existing output path: truncate it to zero, grow it again,
    // then fills most (but not necessarily every reserved byte) through a
    // writable shared mapping.  Bytes made visible only by the regrowth must
    // be zero, never stale data from the previous incarnation of the file.
    let reuse_fd = open(REUSE_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o755);
    assert!(reuse_fd >= 0, "reuse create failed: {}", reuse_fd);
    let reuse_fd = reuse_fd as usize;
    let mut stale = [0x6eu8; 4096];
    for (index, byte) in stale.iter_mut().enumerate() {
        *byte = 0x41 ^ (index as u8).wrapping_mul(37);
    }
    assert_eq!(write(reuse_fd, &stale), stale.len() as isize);
    assert_eq!(close(reuse_fd), 0);

    let reuse_fd = open(REUSE_PATH, O_TRUNC | O_RDWR, 0o755);
    assert!(reuse_fd >= 0, "reuse truncate-open failed: {}", reuse_fd);
    let reuse_fd = reuse_fd as usize;
    assert_eq!(ftruncate(reuse_fd, 4096), 0);
    let reused = mmap(0, 4096, 0x3, 0x1, reuse_fd as isize, 0);
    assert!(reused >= 0, "reuse mmap failed: {}", reused);
    unsafe {
        core::slice::from_raw_parts_mut(reused as *mut u8, 4096)[24..48].fill(0xa5);
    }
    assert_eq!(munmap(reused as usize, 4096), 0);
    assert_eq!(close(reuse_fd), 0);

    let reuse_fd = open(REUSE_PATH, O_RDONLY, 0);
    assert!(reuse_fd >= 0, "reuse reopen failed: {}", reuse_fd);
    let reuse_fd = reuse_fd as usize;
    let mut reused_data = [0u8; 64];
    assert_eq!(pread(reuse_fd, &mut reused_data, 0), 64);
    assert!(
        reused_data[..24].iter().all(|&byte| byte == 0),
        "truncate/regrow exposed stale prefix"
    );
    assert!(reused_data[24..48].iter().all(|&byte| byte == 0xa5));
    assert!(
        reused_data[48..].iter().all(|&byte| byte == 0),
        "truncate/regrow exposed stale suffix"
    );
    assert_eq!(close(reuse_fd), 0);
    assert_eq!(unlink(REUSE_PATH), 0);

    // A linker can keep a shared output mapping while also issuing pwrite on
    // the same page.  Both access paths must observe one coherent page; an
    // mmap writeback must not restore its stale snapshot over the pwrite.
    let mixed_fd = open(REUSE_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o755);
    assert!(mixed_fd >= 0, "mixed create failed: {}", mixed_fd);
    let mixed_fd = mixed_fd as usize;
    assert_eq!(ftruncate(mixed_fd, 4096), 0);
    let mixed = mmap(0, 4096, 0x3, 0x1, mixed_fd as isize, 0);
    assert!(mixed >= 0, "mixed mmap failed: {}", mixed);
    let direct = [0x3cu8; 24];
    assert_eq!(pwrite(mixed_fd, &direct, 0), direct.len() as isize);
    unsafe {
        core::slice::from_raw_parts_mut(mixed as *mut u8, 4096)[24..48].fill(0xa7);
    }
    assert_eq!(munmap(mixed as usize, 4096), 0);
    assert_eq!(close(mixed_fd), 0);

    let mixed_fd = open(REUSE_PATH, O_RDONLY, 0);
    assert!(mixed_fd >= 0, "mixed reopen failed: {}", mixed_fd);
    let mixed_fd = mixed_fd as usize;
    let mut mixed_data = [0u8; 48];
    assert_eq!(pread(mixed_fd, &mut mixed_data, 0), 48);
    assert!(
        mixed_data[..24].iter().all(|&byte| byte == 0x3c),
        "MAP_SHARED writeback overwrote pwrite data"
    );
    assert!(mixed_data[24..].iter().all(|&byte| byte == 0xa7));
    assert_eq!(close(mixed_fd), 0);
    assert_eq!(unlink(REUSE_PATH), 0);

    // lld grows a brand-new sparse output to its final size and immediately
    // maps it.  Every untouched hole byte must already be zero in that first
    // mapping; lwext4 may otherwise leave bytes from its internal read buffer.
    let sparse_fd = open(REUSE_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o755);
    assert!(sparse_fd >= 0, "sparse create failed: {}", sparse_fd);
    let sparse_fd = sparse_fd as usize;
    assert_eq!(ftruncate(sparse_fd, FINAL_SIZE), 0);
    let sparse = mmap(0, MAP_SIZE, 0x3, 0x1, sparse_fd as isize, 0);
    assert!(sparse >= 0, "sparse mmap failed: {}", sparse);
    let sparse_data = unsafe { core::slice::from_raw_parts(sparse as *const u8, MAP_SIZE) };
    assert!(
        sparse_data[0xbb4d0..0xbb520].iter().all(|&byte| byte == 0),
        "fresh sparse mmap contains nonzero hole data"
    );
    assert_eq!(munmap(sparse as usize, MAP_SIZE), 0);
    assert_eq!(close(sparse_fd), 0);
    assert_eq!(unlink(REUSE_PATH), 0);

    let fd = open(OLD_PATH, O_CREATE | O_TRUNC | O_RDWR, 0o755);
    assert!(fd >= 0, "create failed: {}", fd);
    let fd = fd as usize;

    let mut page = [0u8; 4096];
    let mut offset = 0usize;
    while offset < PREFIX_END {
        let len = (PREFIX_END - offset).min(page.len());
        fill_pattern(&mut page[..len], offset);
        let n = write(fd, &page[..len]);
        assert_eq!(n, len as isize, "write offset={:#x}", offset);
        offset += len;
    }

    assert_eq!(
        pwrite(fd, &sections, SECTION_OFFSET as isize),
        SECTION_LEN as isize
    );

    // Cargo persists a hashed artifact under its final name using a hard link
    // when the filesystem supports it.  Exercise the strongest lifetime case:
    // link and unlink the source while its dirty open-file description lives.
    assert_eq!(link(OLD_PATH, NEW_PATH), 0);
    assert_eq!(unlink(OLD_PATH), 0);
    assert_eq!(close(fd), 0);

    let fd = open(NEW_PATH, O_RDONLY, 0);
    assert!(fd >= 0, "reopen failed: {}", fd);
    let fd = fd as usize;
    let mut stat = user_lib::Stat::default();
    assert_eq!(fstat(fd, &mut stat), 0);
    assert_eq!(stat.st_size as usize, FINAL_SIZE);

    verify_pattern(fd, 0, page.len(), &mut page);
    verify_pattern(fd, 0x1000, page.len(), &mut page);
    verify_zero(fd, 0x200000, page.len(), &mut page);
    verify_zero(fd, 0x40c000, SECTION_OFFSET - 0x40c000, &mut page);

    let mut actual_sections = [0u8; SECTION_LEN];
    assert_eq!(
        pread(fd, &mut actual_sections, SECTION_OFFSET as isize),
        SECTION_LEN as isize
    );
    assert_eq!(actual_sections, sections, "section-header tail mismatch");
    assert_eq!(close(fd), 0);

    let _ = unlink(NEW_PATH);
    println!("BUILDSTORM_FILE_PROBE_PASS");
    0
}
