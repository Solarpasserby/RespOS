#define _GNU_SOURCE

#include <assert.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/xattr.h>
#include <unistd.h>

#define PATH "/dev/shm/respos-xattr-probe"
#define ALIAS "/dev/shm/respos-xattr-alias"
#define NAME "user.respos"

static void cleanup(void) {
    (void)unlink(PATH);
    (void)unlink(ALIAS);
}

static void read_value(const char *path, char value[7]) {
    assert(getxattr(path, NAME, value, 7) == 7);
}

int main(void) {
    cleanup();
    int fd = open(PATH, O_CREAT | O_TRUNC | O_RDWR, 0640);
    assert(fd >= 0);
    assert(setxattr(PATH, NAME, "initial", 7, XATTR_CREATE) == 0);
    assert(link(PATH, ALIAS) == 0);
    char value[7];
    read_value(ALIAS, value);
    assert(memcmp(value, "initial", 7) == 0);
    assert(fsetxattr(fd, NAME, "replace", 7, XATTR_REPLACE) == 0);
    assert(unlink(PATH) == 0);
    assert(unlink(ALIAS) == 0);
    assert(fgetxattr(fd, NAME, value, sizeof(value)) == 7);
    assert(memcmp(value, "replace", 7) == 0);
    assert(close(fd) == 0);
    puts("FS_XATTR_PROBE_PASS");
    return 0;
}
