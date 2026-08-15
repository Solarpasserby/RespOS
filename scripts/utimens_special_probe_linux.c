#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static int timespec_equal(struct timespec left, struct timespec right)
{
    return left.tv_sec == right.tv_sec && left.tv_nsec == right.tv_nsec;
}

static int timespec_compare(struct timespec left, struct timespec right)
{
    if (left.tv_sec != right.tv_sec)
        return left.tv_sec < right.tv_sec ? -1 : 1;
    if (left.tv_nsec != right.tv_nsec)
        return left.tv_nsec < right.tv_nsec ? -1 : 1;
    return 0;
}

static struct stat file_stat(const char *path)
{
    struct stat value;
    assert(stat(path, &value) == 0);
    return value;
}

int main(void)
{
    setbuf(stdout, NULL);
    char path[] = "/tmp/respos-utimens-special-XXXXXX";
    int fd = mkstemp(path);
    assert(fd >= 0);
    assert(close(fd) == 0);

    struct timespec times[2] = {
        { .tv_sec = 100, .tv_nsec = 0 },
        { .tv_sec = 200, .tv_nsec = 0 },
    };
    assert(utimensat(AT_FDCWD, path, times, 0) == 0);
    struct stat baseline = file_stat(path);
    assert(baseline.st_atim.tv_sec == 100);
    assert(baseline.st_mtim.tv_sec == 200);

    times[0] = (struct timespec){ .tv_sec = LLONG_MIN, .tv_nsec = UTIME_OMIT };
    times[1] = (struct timespec){ .tv_sec = LLONG_MAX, .tv_nsec = UTIME_OMIT };
    assert(utimensat(AT_FDCWD, path, times, 0) == 0);
    struct stat omitted = file_stat(path);
    assert(timespec_equal(omitted.st_atim, baseline.st_atim));
    assert(timespec_equal(omitted.st_mtim, baseline.st_mtim));
    assert(timespec_equal(omitted.st_ctim, baseline.st_ctim));

    char missing[sizeof(path) + 16];
    assert(snprintf(missing, sizeof(missing), "%s-missing", path) > 0);
    assert(utimensat(AT_FDCWD, missing, times, 0) == 0);

    times[0] = (struct timespec){ .tv_sec = LLONG_MIN, .tv_nsec = UTIME_OMIT };
    times[1] = (struct timespec){ .tv_sec = 300, .tv_nsec = 0 };
    assert(utimensat(AT_FDCWD, path, times, 0) == 0);
    struct stat one_omitted = file_stat(path);
    assert(timespec_equal(one_omitted.st_atim, baseline.st_atim));
    assert(one_omitted.st_mtim.tv_sec == 300 && one_omitted.st_mtim.tv_nsec == 0);

    struct timespec before_now;
    struct timespec after_now;
    assert(clock_gettime(CLOCK_REALTIME, &before_now) == 0);
    times[0] = (struct timespec){ .tv_sec = LLONG_MAX, .tv_nsec = UTIME_NOW };
    times[1] = (struct timespec){ .tv_sec = LLONG_MIN, .tv_nsec = UTIME_OMIT };
    assert(utimensat(AT_FDCWD, path, times, 0) == 0);
    assert(clock_gettime(CLOCK_REALTIME, &after_now) == 0);
    struct stat now_and_omit = file_stat(path);
    struct timespec rounded_before = {
        .tv_sec = before_now.tv_sec - 1,
        .tv_nsec = 0,
    };
    assert(timespec_compare(now_and_omit.st_atim, rounded_before) >= 0);
    assert(timespec_compare(now_and_omit.st_atim, after_now) <= 0);
    assert(timespec_equal(now_and_omit.st_mtim, one_omitted.st_mtim));

    struct stat before_invalid = file_stat(path);
    times[0] = (struct timespec){ .tv_sec = 400, .tv_nsec = 1000000000L };
    times[1] = (struct timespec){ .tv_sec = LLONG_MIN, .tv_nsec = UTIME_OMIT };
    errno = 0;
    assert(utimensat(AT_FDCWD, path, times, 0) == -1);
    assert(errno == EINVAL);
    struct stat after_invalid = file_stat(path);
    assert(timespec_equal(after_invalid.st_atim, before_invalid.st_atim));
    assert(timespec_equal(after_invalid.st_mtim, before_invalid.st_mtim));
    assert(timespec_equal(after_invalid.st_ctim, before_invalid.st_ctim));

    times[0] = (struct timespec){ .tv_sec = LLONG_MAX, .tv_nsec = UTIME_NOW };
    times[1] = (struct timespec){ .tv_sec = 500, .tv_nsec = -1 };
    errno = 0;
    assert(utimensat(AT_FDCWD, path, times, 0) == -1);
    assert(errno == EINVAL);
    after_invalid = file_stat(path);
    assert(timespec_equal(after_invalid.st_atim, before_invalid.st_atim));
    assert(timespec_equal(after_invalid.st_mtim, before_invalid.st_mtim));
    assert(timespec_equal(after_invalid.st_ctim, before_invalid.st_ctim));

    assert(unlink(path) == 0);
    puts("UTIMENS_SPECIAL_LINUX PASS omit=pass now=pass invalid_nsec=pass missing_double_omit=pass");
    return 0;
}
