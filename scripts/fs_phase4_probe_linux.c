#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static void expect_errno(int result, int expected) {
    assert(result == -1);
    assert(errno == expected);
}

static struct stat stat_path(const char *path, int flags) {
    struct stat value;
    assert(fstatat(AT_FDCWD, path, &value, flags) == 0);
    return value;
}

int main(void) {
    char root[] = "/tmp/respos-phase4-XXXXXX";
    assert(mkdtemp(root) != NULL);

    char target[256], target_slash[256], symlink_path[256], symlink_slash[256];
    char hard_link[256], follow_link[256], renamed_link[256], missing_slash[256];
    snprintf(target, sizeof(target), "%s/target", root);
    snprintf(target_slash, sizeof(target_slash), "%s/target/", root);
    snprintf(symlink_path, sizeof(symlink_path), "%s/link", root);
    snprintf(symlink_slash, sizeof(symlink_slash), "%s/link/", root);
    snprintf(hard_link, sizeof(hard_link), "%s/hard-link", root);
    snprintf(follow_link, sizeof(follow_link), "%s/follow-link", root);
    snprintf(renamed_link, sizeof(renamed_link), "%s/renamed-link", root);
    snprintf(missing_slash, sizeof(missing_slash), "%s/missing/", root);

    int fd = open(target, O_CREAT | O_TRUNC | O_RDWR, 0644);
    assert(fd >= 0);
    assert(write(fd, "target", 6) == 6);
    assert(close(fd) == 0);
    assert(symlink("target", symlink_path) == 0);

    struct stat target_stat = stat_path(target, 0);
    struct stat link_stat = stat_path(symlink_path, AT_SYMLINK_NOFOLLOW);
    assert(target_stat.st_ino != link_stat.st_ino);

    fd = open(symlink_path, O_PATH | O_NOFOLLOW);
    assert(fd >= 0);
    struct stat fd_stat;
    assert(fstat(fd, &fd_stat) == 0 && fd_stat.st_ino == link_stat.st_ino);
    assert(close(fd) == 0);
    expect_errno(open(symlink_slash, O_PATH | O_NOFOLLOW), ENOTDIR);

    assert(link(symlink_path, hard_link) == 0);
    assert(stat_path(hard_link, AT_SYMLINK_NOFOLLOW).st_ino == link_stat.st_ino);
    assert(linkat(AT_FDCWD, symlink_path, AT_FDCWD, follow_link,
                  AT_SYMLINK_FOLLOW) == 0);
    assert(stat_path(follow_link, 0).st_ino == target_stat.st_ino);
    assert(rename(symlink_path, renamed_link) == 0);
    assert(stat_path(renamed_link, AT_SYMLINK_NOFOLLOW).st_ino == link_stat.st_ino);
    assert(stat_path(target, 0).st_ino == target_stat.st_ino);

    expect_errno(open(target_slash, O_RDONLY), ENOTDIR);
    expect_errno(open(missing_slash, O_CREAT | O_RDWR, 0600), EISDIR);
    expect_errno(unlink(target_slash), ENOTDIR);
    char renamed_slash[256];
    snprintf(renamed_slash, sizeof(renamed_slash), "%s/renamed-link/", root);
    expect_errno(rename(renamed_slash, symlink_path), ENOTDIR);
    expect_errno(rename(target, missing_slash), ENOTDIR);
    expect_errno(link(target, missing_slash), ENOENT);

    assert(chmod(target, 0) == 0);
    expect_errno(open(target, O_RDONLY), EACCES);
    fd = open(target, O_PATH | O_CLOEXEC);
    assert(fd >= 0);
    assert(fcntl(fd, F_GETFD) == FD_CLOEXEC);
    assert((fcntl(fd, F_GETFL) & (O_PATH | O_APPEND | O_NONBLOCK)) == O_PATH);
    char byte;
    expect_errno((int)read(fd, &byte, 1), EBADF);
    expect_errno(fchmod(fd, 0600), EBADF);
    struct stat empty_stat;
    assert(fstatat(fd, "", &empty_stat, AT_EMPTY_PATH) == 0);
    assert(empty_stat.st_ino == target_stat.st_ino);
    assert(close(fd) == 0);
    assert(chmod(target, 0640) == 0);

    fd = open(target, O_RDWR | O_APPEND | O_CLOEXEC);
    assert(fd >= 0);
    int duplicate = dup(fd);
    assert(duplicate >= 0);
    assert(fcntl(fd, F_GETFD) == FD_CLOEXEC);
    assert(fcntl(duplicate, F_GETFD) == 0);
    assert(fcntl(duplicate, F_SETFL, O_NONBLOCK) == 0);
    int shared_flags = fcntl(fd, F_GETFL);
    assert((shared_flags & O_NONBLOCK) != 0);
    assert((shared_flags & O_APPEND) == 0);
    assert(close(duplicate) == 0);
    assert(close(fd) == 0);

    mode_t old_mask = umask(0027);
    char mode_file[256];
    snprintf(mode_file, sizeof(mode_file), "%s/mode-file", root);
    fd = open(mode_file, O_CREAT | O_EXCL | O_RDWR, 0666);
    assert(fd >= 0);
    assert(close(fd) == 0);
    assert((stat_path(mode_file, 0).st_mode & 0777) == 0640);
    umask(old_mask);

    assert(unlink(mode_file) == 0);
    assert(unlink(renamed_link) == 0);
    assert(unlink(follow_link) == 0);
    assert(unlink(hard_link) == 0);
    assert(unlink(target) == 0);
    assert(rmdir(root) == 0);
    puts("FS_PHASE4_LINUX_PROBE_PASS");
    return 0;
}
