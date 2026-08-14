#define _GNU_SOURCE

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static void expect_errno(const char *path, int expected, const char *what)
{
    errno = 0;
    int result = chroot(path);
    if (result != -1 || errno != expected) {
        fprintf(stderr, "FAIL: %s result=%d errno=%d expected=%d\n",
                what, result, errno, expected);
        exit(1);
    }
}

int main(void)
{
    char base[] = "/tmp/respos-chroot-perm-XXXXXX";
    char blocked[sizeof(base) + 16];
    char accessible[sizeof(base) + 16];
    char missing[sizeof(base) + 16];

    if (mkdtemp(base) == NULL) {
        perror("mkdtemp");
        return 1;
    }
    snprintf(blocked, sizeof(blocked), "%s/blocked", base);
    snprintf(accessible, sizeof(accessible), "%s/accessible", base);
    snprintf(missing, sizeof(missing), "%s/missing", base);
    if (mkdir(blocked, 0000) != 0 || mkdir(accessible, 0700) != 0) {
        perror("mkdir");
        return 1;
    }

    expect_errno(blocked, EACCES, "search permission precedes CAP_SYS_CHROOT");
    expect_errno(missing, ENOENT, "lookup precedes CAP_SYS_CHROOT");
    expect_errno(accessible, EPERM, "accessible directory still requires CAP_SYS_CHROOT");

    if (rmdir(blocked) != 0 || rmdir(accessible) != 0 || rmdir(base) != 0) {
        perror("cleanup");
        return 1;
    }
    puts("CHROOT_PERMISSION_LINUX_PASS");
    return 0;
}
