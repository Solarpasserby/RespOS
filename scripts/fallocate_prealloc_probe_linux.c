#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef FALLOC_FL_KEEP_SIZE
#define FALLOC_FL_KEEP_SIZE 0x01
#endif

#define BLOCK_SIZE 4096

static void fail(const char *what)
{
    perror(what);
    exit(1);
}

static void expect(int condition, const char *what)
{
    if (!condition) {
        fprintf(stderr, "FAIL: %s\n", what);
        exit(1);
    }
}

static void expect_zero_range(int fd, off_t offset, size_t len, const char *what)
{
    unsigned char buf[BLOCK_SIZE];

    while (len != 0) {
        size_t chunk = len < sizeof(buf) ? len : sizeof(buf);
        ssize_t n = pread(fd, buf, chunk, offset);

        if (n < 0)
            fail("pread(zero range)");
        expect((size_t)n == chunk, what);
        for (size_t i = 0; i < chunk; i++)
            expect(buf[i] == 0, what);
        offset += n;
        len -= (size_t)n;
    }
}

static void check_keep_size(void)
{
    char path[] = "/tmp/respos-fallocate-keep-XXXXXX";
    unsigned char data[BLOCK_SIZE];
    struct stat before;
    struct stat allocated;
    struct stat grown;
    off_t old_size = 4 * BLOCK_SIZE;
    off_t alloc_offset = 8 * BLOCK_SIZE;
    size_t alloc_len = 2 * BLOCK_SIZE;
    int fd = mkstemp(path);

    if (fd < 0)
        fail("mkstemp(keep-size)");
    memset(data, 0x5a, sizeof(data));
    if (pwrite(fd, data, sizeof(data), 0) != (ssize_t)sizeof(data))
        fail("pwrite(keep-size payload)");
    if (ftruncate(fd, old_size) < 0)
        fail("ftruncate(keep-size initial)");
    if (fstat(fd, &before) < 0)
        fail("fstat(keep-size before)");
    if (lseek(fd, 123, SEEK_SET) != 123)
        fail("lseek(keep-size before)");

    if (fallocate(fd, FALLOC_FL_KEEP_SIZE, alloc_offset, alloc_len) < 0)
        fail("fallocate(FALLOC_FL_KEEP_SIZE)");
    if (fstat(fd, &allocated) < 0)
        fail("fstat(keep-size allocated)");
    expect(allocated.st_size == old_size,
           "FALLOC_FL_KEEP_SIZE must preserve logical size");
    expect(allocated.st_blocks > before.st_blocks,
           "FALLOC_FL_KEEP_SIZE must reserve physical blocks");
    expect(lseek(fd, 0, SEEK_CUR) == 123,
           "fallocate must preserve the open-file offset");

    if (ftruncate(fd, alloc_offset + (off_t)alloc_len) < 0)
        fail("ftruncate(keep-size reveal)");
    if (fstat(fd, &grown) < 0)
        fail("fstat(keep-size grown)");
    expect(grown.st_blocks >= allocated.st_blocks,
           "growing the file must retain preallocated blocks");
    expect_zero_range(fd, alloc_offset, alloc_len,
                      "preallocated unwritten range must read as zero");
    if (pread(fd, data, sizeof(data), 0) != (ssize_t)sizeof(data))
        fail("pread(keep-size payload)");
    for (size_t i = 0; i < sizeof(data); i++)
        expect(data[i] == 0x5a, "preallocation must preserve existing data");

    if (close(fd) < 0)
        fail("close(keep-size)");
    if (unlink(path) < 0)
        fail("unlink(keep-size)");
}

static void check_default_mode(void)
{
    char path[] = "/tmp/respos-fallocate-default-XXXXXX";
    unsigned char data[BLOCK_SIZE];
    struct stat before;
    struct stat after;
    off_t old_size = BLOCK_SIZE;
    off_t alloc_offset = 4 * BLOCK_SIZE;
    size_t alloc_len = 2 * BLOCK_SIZE;
    int fd = mkstemp(path);

    if (fd < 0)
        fail("mkstemp(default)");
    memset(data, 0xa5, sizeof(data));
    if (write(fd, data, sizeof(data)) != (ssize_t)sizeof(data))
        fail("write(default payload)");
    if (fstat(fd, &before) < 0)
        fail("fstat(default before)");
    if (lseek(fd, 111, SEEK_SET) != 111)
        fail("lseek(default before)");

    if (fallocate(fd, 0, alloc_offset, alloc_len) < 0)
        fail("fallocate(default)");
    if (fstat(fd, &after) < 0)
        fail("fstat(default after)");
    expect(after.st_size == alloc_offset + (off_t)alloc_len,
           "default fallocate must extend the logical size to range end");
    expect(after.st_blocks > before.st_blocks,
           "default fallocate must reserve physical blocks");
    expect(lseek(fd, 0, SEEK_CUR) == 111,
           "default fallocate must preserve the open-file offset");
    expect_zero_range(fd, old_size,
                      (size_t)(alloc_offset + (off_t)alloc_len - old_size),
                      "new sparse gap and allocated range must read as zero");
    if (pread(fd, data, sizeof(data), 0) != (ssize_t)sizeof(data))
        fail("pread(default payload)");
    for (size_t i = 0; i < sizeof(data); i++)
        expect(data[i] == 0xa5, "default fallocate must preserve existing data");

    if (close(fd) < 0)
        fail("close(default)");
    if (unlink(path) < 0)
        fail("unlink(default)");
}

int main(void)
{
    check_keep_size();
    check_default_mode();
    puts("fallocate preallocation Linux probe: PASS");
    return 0;
}
