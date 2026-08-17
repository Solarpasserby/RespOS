#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static void fail(const char *what)
{
    perror(what);
    exit(1);
}

static void verify(const char *path)
{
    struct stat st;

    if (stat(path, &st) != 0)
        fail("stat");
    if (st.st_atim.tv_sec != (time_t)-1 || st.st_atim.tv_nsec != 123456789L ||
        st.st_mtim.tv_sec != (time_t)2147483648LL ||
        st.st_mtim.tv_nsec != 987654321L) {
        fprintf(stderr,
                "timestamp mismatch: atime=%lld.%09ld mtime=%lld.%09ld\n",
                (long long)st.st_atim.tv_sec, st.st_atim.tv_nsec,
                (long long)st.st_mtim.tv_sec, st.st_mtim.tv_nsec);
        exit(1);
    }
}

int main(void)
{
    static const char path[] = "/tmp/respos_ext4_timestamp_phase5.XXXXXX";
    char writable_path[sizeof(path)];
    struct timespec times[2] = {
        {.tv_sec = (time_t)-1, .tv_nsec = 123456789L},
        {.tv_sec = (time_t)2147483648LL, .tv_nsec = 987654321L},
    };
    int fd;

    for (size_t i = 0; i < sizeof(path); ++i)
        writable_path[i] = path[i];
    fd = mkstemp(writable_path);
    if (fd < 0)
        fail("mkstemp");
    if (futimens(fd, times) != 0)
        fail("futimens");
    verify(writable_path);
    if (close(fd) != 0)
        fail("close");
    fd = open(writable_path, O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        fail("reopen");
    verify(writable_path);
    if (close(fd) != 0)
        fail("close reopened");
    if (unlink(writable_path) != 0)
        fail("unlink");

    puts("EXT4_TIMESTAMP_LINUX PASS negative_sec=pass epoch=pass nsec=pass reopen=pass");
    return 0;
}
