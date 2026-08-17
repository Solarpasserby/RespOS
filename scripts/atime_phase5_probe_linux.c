#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
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

static struct stat get_stat(const char *path)
{
    struct stat value;
    if (stat(path, &value) != 0)
        fail("stat");
    return value;
}

static void set_times(const char *path, time_t atime, time_t mtime)
{
    struct timespec times[2] = {
        {.tv_sec = atime, .tv_nsec = 111222333L},
        {.tv_sec = mtime, .tv_nsec = 444555666L},
    };
    if (utimensat(AT_FDCWD, path, times, 0) != 0)
        fail("utimensat");
}

static void read_byte(int fd)
{
    char byte;
    if (lseek(fd, 0, SEEK_SET) != 0)
        fail("lseek");
    if (read(fd, &byte, 1) != 1)
        fail("read");
}

static void read_directory(const char *path, int flags)
{
    struct dirent *entry;
    DIR *dir;
    int fd = open(path, O_RDONLY | O_DIRECTORY | O_CLOEXEC | flags);

    if (fd < 0)
        fail("open directory");
    dir = fdopendir(fd);
    if (dir == NULL) {
        close(fd);
        fail("fdopendir");
    }
    errno = 0;
    do {
        entry = readdir(dir);
    } while (entry != NULL);
    if (errno != 0)
        fail("readdir");
    if (closedir(dir) != 0)
        fail("closedir");
}

static int same_time(struct timespec a, struct timespec b)
{
    return a.tv_sec == b.tv_sec && a.tv_nsec == b.tv_nsec;
}

int main(void)
{
    char path[] = "/tmp/respos_atime_phase5.XXXXXX";
    char dir_path[] = "/tmp/respos_atime_dir_phase5.XXXXXX";
    struct stat before, after, repeated;
    time_t now = time(NULL);
    int fd = mkstemp(path);
    char *dir;

    if (fd < 0)
        fail("mkstemp");
    if (write(fd, "x", 1) != 1)
        fail("write");
    dir = mkdtemp(dir_path);
    if (dir == NULL)
        fail("mkdtemp");

    set_times(path, now + 3600, now - 3600);
    before = get_stat(path);
    read_byte(fd);
    after = get_stat(path);
    if (!same_time(before.st_atim, after.st_atim) ||
        !same_time(before.st_ctim, after.st_ctim)) {
        fprintf(stderr, "relatime suppression or ctime preservation failed\n");
        return 1;
    }

    set_times(path, 100, 200);
    before = get_stat(path);
    read_byte(fd);
    after = get_stat(path);
    if (after.st_atim.tv_sec < now || !same_time(before.st_ctim, after.st_ctim)) {
        fprintf(stderr, "relatime update or ctime preservation failed\n");
        return 1;
    }
    read_byte(fd);
    repeated = get_stat(path);
    if (!same_time(after.st_atim, repeated.st_atim) ||
        !same_time(after.st_ctim, repeated.st_ctim)) {
        fprintf(stderr, "repeated relatime read changed metadata\n");
        return 1;
    }

    if (close(fd) != 0)
        fail("close");
    set_times(path, 100, 200);
    before = get_stat(path);
    fd = open(path, O_RDONLY | O_NOATIME | O_CLOEXEC);
    if (fd < 0)
        fail("open O_NOATIME");
    read_byte(fd);
    after = get_stat(path);
    if (!same_time(before.st_atim, after.st_atim) ||
        !same_time(before.st_ctim, after.st_ctim)) {
        fprintf(stderr, "O_NOATIME changed atime or ctime\n");
        return 1;
    }

    set_times(dir, now + 3600, now - 3600);
    before = get_stat(dir);
    read_directory(dir, 0);
    after = get_stat(dir);
    if (!same_time(before.st_atim, after.st_atim) ||
        !same_time(before.st_ctim, after.st_ctim)) {
        fprintf(stderr, "directory relatime suppression failed\n");
        return 1;
    }

    set_times(dir, 100, 200);
    before = get_stat(dir);
    read_directory(dir, 0);
    after = get_stat(dir);
    if (after.st_atim.tv_sec < now || !same_time(before.st_ctim, after.st_ctim)) {
        fprintf(stderr, "directory relatime update failed\n");
        return 1;
    }
    read_directory(dir, 0);
    repeated = get_stat(dir);
    if (!same_time(after.st_atim, repeated.st_atim) ||
        !same_time(after.st_ctim, repeated.st_ctim)) {
        fprintf(stderr, "repeated directory read changed metadata\n");
        return 1;
    }

    set_times(dir, 100, 200);
    before = get_stat(dir);
    read_directory(dir, O_NOATIME);
    after = get_stat(dir);
    if (!same_time(before.st_atim, after.st_atim) ||
        !same_time(before.st_ctim, after.st_ctim)) {
        fprintf(stderr, "directory O_NOATIME changed metadata\n");
        return 1;
    }

    if (close(fd) != 0 || unlink(path) != 0 || rmdir(dir) != 0)
        fail("cleanup");
    puts("ATIME_PHASE5_LINUX PASS relatime=pass repeated=pass directory=pass ctime=pass noatime=pass");
    return 0;
}
