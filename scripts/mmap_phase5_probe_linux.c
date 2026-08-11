#define _GNU_SOURCE

#include <assert.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

enum { PAGE_SIZE_ = 4096, MAP_SIZE = 3 * PAGE_SIZE_ };

static void expect_sigbus(volatile unsigned char *address)
{
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        volatile unsigned char value = *address;
        (void)value;
        _exit(99);
    }

    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFSIGNALED(status));
    assert(WTERMSIG(status) == SIGBUS);
}

static void write_byte(int fd, off_t offset, unsigned char value)
{
    assert(pwrite(fd, &value, 1, offset) == 1);
}

static void test_mode(int fd, int map_flag, const char *label)
{
    assert(ftruncate(fd, PAGE_SIZE_ + 128) == 0);
    write_byte(fd, PAGE_SIZE_ + 64, 0x5a);
    unsigned char *mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                  map_flag, fd, 0);
    assert(mapping != MAP_FAILED);
    assert(mapping[PAGE_SIZE_ + 64] == 0x5a);
    assert(mapping[PAGE_SIZE_ + 512] == 0);
    expect_sigbus(mapping + 2 * PAGE_SIZE_);
    assert(munmap(mapping, MAP_SIZE) == 0);

    assert(ftruncate(fd, MAP_SIZE) == 0);
    write_byte(fd, PAGE_SIZE_ + 512, 0x7c);
    write_byte(fd, 2 * PAGE_SIZE_ + 17, 0x6d);
    mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE, map_flag, fd, 0);
    assert(mapping != MAP_FAILED);
    assert(mapping[PAGE_SIZE_ + 512] == 0x7c);
    assert(mapping[2 * PAGE_SIZE_ + 17] == 0x6d);
    assert(ftruncate(fd, PAGE_SIZE_ + 128) == 0);
    assert(mapping[PAGE_SIZE_ + 512] == 0);
    expect_sigbus(mapping + 2 * PAGE_SIZE_ + 17);
    assert(munmap(mapping, MAP_SIZE) == 0);

    assert(ftruncate(fd, PAGE_SIZE_) == 0);
    mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE, map_flag, fd, 0);
    assert(mapping != MAP_FAILED);
    assert(ftruncate(fd, MAP_SIZE) == 0);
    write_byte(fd, 2 * PAGE_SIZE_ + 33, 0xa7);
    assert(mapping[2 * PAGE_SIZE_ + 33] == 0xa7);
    assert(munmap(mapping, MAP_SIZE) == 0);

    printf("MMAP_PHASE5_LINUX %s PASS\n", label);
}

static void test_private_cow_truncate(int fd)
{
    assert(ftruncate(fd, MAP_SIZE) == 0);
    unsigned char *mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE, fd, 0);
    assert(mapping != MAP_FAILED);
    mapping[PAGE_SIZE_ + 64] = 0x44;
    mapping[PAGE_SIZE_ + 512] = 0x55;
    mapping[2 * PAGE_SIZE_ + 17] = 0x66;
    assert(ftruncate(fd, PAGE_SIZE_ + 128) == 0);
    assert(mapping[PAGE_SIZE_ + 64] == 0x44);
    /* A private COW page keeps its anonymous bytes even past partial EOF. */
    assert(mapping[PAGE_SIZE_ + 512] == 0x55);
    /* Whole pages beyond the new EOF are invalidated even after COW. */
    expect_sigbus(mapping + 2 * PAGE_SIZE_ + 17);
    assert(munmap(mapping, MAP_SIZE) == 0);
    puts("MMAP_PHASE5_LINUX private_cow_truncate PASS");
}

int main(void)
{
    setbuf(stdout, NULL);
    char path[] = "/tmp/respos-mmap-phase5-XXXXXX";
    int fd = mkstemp(path);
    assert(fd >= 0);
    assert(unlink(path) == 0);

    test_mode(fd, MAP_SHARED, "shared");
    assert(ftruncate(fd, 0) == 0);
    test_mode(fd, MAP_PRIVATE, "private");
    test_private_cow_truncate(fd);

    assert(close(fd) == 0);
    puts("MMAP_PHASE5_LINUX ALL PASS");
    return 0;
}
