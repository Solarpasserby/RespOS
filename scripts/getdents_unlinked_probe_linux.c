#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <unistd.h>

static void fail(const char *what)
{
    perror(what);
    exit(1);
}

static void expect_errno(long result, int expected, const char *what)
{
    if (result != -1 || errno != expected) {
        fprintf(stderr, "FAIL: %s result=%ld errno=%d expected=%d\n",
                what, result, errno, expected);
        exit(1);
    }
}

static void test_unlinked_directory(int prime_directory_stream)
{
    char path[] = "/tmp/respos-getdents-unlinked-XXXXXX";
    unsigned char buffer[512];
    long result;
    int fd;

    if (mkdtemp(path) == NULL)
        fail("mkdtemp");
    fd = open(path, O_RDONLY | O_DIRECTORY);
    if (fd < 0)
        fail("open(directory)");

    if (prime_directory_stream) {
        result = syscall(SYS_getdents64, fd, buffer, sizeof(buffer));
        if (result <= 0)
            fail("getdents64(prime)");
        if (lseek(fd, 0, SEEK_SET) != 0)
            fail("lseek(directory)");
    } else {
        errno = 0;
        result = syscall(SYS_getdents64, fd, buffer, 1);
        expect_errno(result, EINVAL, "one-byte result buffer");
    }

    if (rmdir(path) < 0)
        fail("rmdir(open directory)");
    errno = 0;
    result = syscall(SYS_getdents64, fd, buffer, sizeof(buffer));
    expect_errno(result, ENOENT,
                 prime_directory_stream
                     ? "unlinked directory after prior getdents"
                     : "unlinked directory before first getdents");
    if (close(fd) < 0)
        fail("close(directory)");
}

int main(void)
{
    test_unlinked_directory(0);
    test_unlinked_directory(1);
    puts("getdents unlinked Linux probe: PASS");
    return 0;
}
