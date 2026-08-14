#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

int main(void)
{
    const unsigned int expected_major = 0xabc;
    const unsigned int expected_minor = 0x54321;
    const dev_t device = makedev(expected_major, expected_minor);
    char path[128];

    assert(major(device) == expected_major);
    assert(minor(device) == expected_minor);
    assert((unsigned long long)device == 0x543abc21ULL);

    snprintf(path, sizeof(path), "/tmp/respos-mknod-dev-%ld", (long)getpid());
    unlink(path);
    if (mknod(path, S_IFCHR | 0600, device) != 0) {
        if (errno == EPERM || errno == EACCES) {
            puts("MKNOD_DEV_T_RUNTIME_SKIPPED_NO_CAP_MKNOD");
            puts("MKNOD_DEV_T_ENCODING_PASS");
            return 0;
        }
        perror("mknod");
        return 1;
    }

    struct stat st = {0};
    struct statx stx = {0};
    assert(stat(path, &st) == 0);
    assert(st.st_rdev == device);
    assert(statx(AT_FDCWD, path, 0, STATX_BASIC_STATS, &stx) == 0);
    assert(stx.stx_rdev_major == expected_major);
    assert(stx.stx_rdev_minor == expected_minor);
    assert(unlink(path) == 0);
    puts("MKNOD_DEV_T_RUNTIME_PASS");
    puts("MKNOD_DEV_T_ENCODING_PASS");
    return 0;
}
