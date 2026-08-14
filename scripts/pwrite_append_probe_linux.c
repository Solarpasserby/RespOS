#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define K1 1024
#define K2 (2 * K1)
#define K3 (3 * K1)

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

int main(void)
{
    char path[] = "/tmp/respos-pwrite-append-XXXXXX";
    char initial[K2];
    char appended[K1];
    char byte;
    struct stat st;
    int flags;
    int fd;

    memset(initial, 'a', sizeof(initial));
    memset(appended, 'b', sizeof(appended));

    fd = mkstemp(path);
    if (fd < 0)
        fail("mkstemp");
    if (write(fd, initial, sizeof(initial)) != (ssize_t)sizeof(initial))
        fail("write(initial)");
    if (close(fd) < 0)
        fail("close(initial)");

    fd = open(path, O_RDWR | O_APPEND);
    if (fd < 0)
        fail("open(O_APPEND)");
    if (lseek(fd, K1, SEEK_SET) != K1)
        fail("lseek(before pwrite)");
    if (pwrite(fd, appended, sizeof(appended), 0) != (ssize_t)sizeof(appended))
        fail("pwrite(O_APPEND)");
    if (fstat(fd, &st) < 0)
        fail("fstat(after append)");
    expect(st.st_size == K3, "O_APPEND pwrite must append at EOF");
    expect(lseek(fd, 0, SEEK_CUR) == K1,
           "pwrite must preserve the open-file offset");
    if (pread(fd, &byte, 1, 0) != 1)
        fail("pread(original)");
    expect(byte == 'a', "O_APPEND pwrite must ignore its explicit offset");
    if (pread(fd, &byte, 1, K2) != 1)
        fail("pread(appended)");
    expect(byte == 'b', "appended payload must start at the old EOF");

    flags = fcntl(fd, F_GETFL);
    if (flags < 0 || fcntl(fd, F_SETFL, flags & ~O_APPEND) < 0)
        fail("clear O_APPEND");
    byte = 'c';
    if (pwrite(fd, &byte, 1, 0) != 1)
        fail("pwrite(positioned)");
    if (fstat(fd, &st) < 0)
        fail("fstat(positioned)");
    expect(st.st_size == K3, "positioned overwrite must not extend the file");
    expect(lseek(fd, 0, SEEK_CUR) == K1,
           "positioned pwrite must preserve the open-file offset");

    if (close(fd) < 0)
        fail("close(final)");
    if (unlink(path) < 0)
        fail("unlink");
    puts("pwrite append Linux probe: PASS");
    return 0;
}
