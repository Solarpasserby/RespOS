#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

enum { PAGE_SIZE_ = 4096 };

static void expect_write(void *address, int expected_signal)
{
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        *(volatile unsigned char *)address = 0x6d;
        _exit(0);
    }

    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    if (expected_signal == 0) {
        assert(WIFEXITED(status));
        assert(WEXITSTATUS(status) == 0);
    } else {
        assert(WIFSIGNALED(status));
        assert(WTERMSIG(status) == expected_signal);
    }
}

static void test_einval_preserves_permissions(void)
{
    unsigned char *mapping = mmap(NULL, 2 * PAGE_SIZE_, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    assert(mapping != MAP_FAILED);
    mapping[0] = 0x11;
    mapping[PAGE_SIZE_] = 0x22;
    assert(mprotect(mapping, PAGE_SIZE_, PROT_READ) == 0);

    errno = 0;
    assert(mprotect(mapping, 2 * PAGE_SIZE_, PROT_READ | PROT_WRITE | 0x40000000) == -1);
    assert(errno == EINVAL);
    expect_write(mapping, SIGSEGV);
    expect_write(mapping + PAGE_SIZE_, 0);

    errno = 0;
    assert(mprotect(mapping + 1, PAGE_SIZE_, PROT_NONE) == -1);
    assert(errno == EINVAL);
    expect_write(mapping, SIGSEGV);
    expect_write(mapping + PAGE_SIZE_, 0);
    assert(munmap(mapping, 2 * PAGE_SIZE_) == 0);
}

static void test_eacces_does_not_grant_write(void)
{
    char path[] = "/tmp/respos-mprotect-failure-XXXXXX";
    int fd = mkstemp(path);
    assert(fd >= 0);
    assert(ftruncate(fd, PAGE_SIZE_) == 0);
    assert(close(fd) == 0);

    fd = open(path, O_RDONLY);
    assert(fd >= 0);
    unsigned char *mapping = mmap(NULL, PAGE_SIZE_, PROT_READ, MAP_SHARED, fd, 0);
    assert(mapping != MAP_FAILED);
    errno = 0;
    assert(mprotect(mapping, PAGE_SIZE_, PROT_READ | PROT_WRITE) == -1);
    assert(errno == EACCES);
    expect_write(mapping, SIGSEGV);

    assert(munmap(mapping, PAGE_SIZE_) == 0);
    assert(close(fd) == 0);
    assert(unlink(path) == 0);
}

static void test_unmapped_hole_errno(void)
{
    unsigned char *mapping = mmap(NULL, 3 * PAGE_SIZE_, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    assert(mapping != MAP_FAILED);
    assert(munmap(mapping + PAGE_SIZE_, PAGE_SIZE_) == 0);

    errno = 0;
    assert(mprotect(mapping, 3 * PAGE_SIZE_, PROT_READ) == -1);
    assert(errno == ENOMEM);

    assert(munmap(mapping, PAGE_SIZE_) == 0);
    assert(munmap(mapping + 2 * PAGE_SIZE_, PAGE_SIZE_) == 0);
}

int main(void)
{
    setbuf(stdout, NULL);
    test_einval_preserves_permissions();
    test_eacces_does_not_grant_write();
    test_unmapped_hole_errno();
    puts("MPROTECT_FAILURE_LINUX PASS einval_atomic=pass eacces_write=pass hole_enomem=pass");
    return 0;
}
