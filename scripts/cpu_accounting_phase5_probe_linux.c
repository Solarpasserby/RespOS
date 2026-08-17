#define _GNU_SOURCE

#include <assert.h>
#include <stdint.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/times.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static uint64_t timeval_us(struct timeval value)
{
    return (uint64_t)value.tv_sec * 1000000 + (uint64_t)value.tv_usec;
}

static uint64_t monotonic_ms(void)
{
    struct timespec now;
    assert(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
    return (uint64_t)now.tv_sec * 1000 + (uint64_t)now.tv_nsec / 1000000;
}

static void burn_user_time(void)
{
    uint64_t deadline = monotonic_ms() + 250;
    volatile uintptr_t value = 1;
    while (monotonic_ms() < deadline) {
        for (size_t index = 0; index < 200000; ++index)
            value = value * 1664525 + 1013904223;
    }
    (void)value;
}

static void exercise_block_io_and_major_fault(void)
{
    char path[] = "/tmp/respos-rusage-XXXXXX";
    int fd = mkstemp(path);
    assert(fd >= 0);
    unsigned char page[4096];
    memset(page, 0x5a, sizeof(page));

    struct rusage before_write, after_write, after_repeat;
    assert(getrusage(RUSAGE_THREAD, &before_write) == 0);
    assert(write(fd, page, sizeof(page)) == (ssize_t)sizeof(page));
    assert(write(fd, page, sizeof(page)) == (ssize_t)sizeof(page));
    assert(getrusage(RUSAGE_THREAD, &after_write) == 0);
    assert(after_write.ru_oublock - before_write.ru_oublock >= 16);
    assert(pwrite(fd, page, sizeof(page), 0) == (ssize_t)sizeof(page));
    assert(getrusage(RUSAGE_THREAD, &after_repeat) == 0);
    assert(after_repeat.ru_oublock == after_write.ru_oublock);

    assert(fsync(fd) == 0);
    assert(posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED) == 0);
    unsigned char *mapping = mmap(NULL, 8192, PROT_READ, MAP_PRIVATE, fd, 0);
    assert(mapping != MAP_FAILED);
    struct rusage before_fault, after_fault;
    assert(getrusage(RUSAGE_THREAD, &before_fault) == 0);
    volatile unsigned char value = mapping[0];
    assert(value == 0x5a);
    assert(getrusage(RUSAGE_THREAD, &after_fault) == 0);
    assert(after_fault.ru_majflt > before_fault.ru_majflt);
    assert(after_fault.ru_inblock > before_fault.ru_inblock);
    assert(munmap(mapping, 8192) == 0);
    assert(close(fd) == 0);
    assert(unlink(path) == 0);
}

static volatile sig_atomic_t handled_signal;

static void signal_handler(int signal_number)
{
    handled_signal = signal_number;
}

static void assert_linux_zero_legacy_rusage(struct rusage usage)
{
    assert(usage.ru_ixrss == 0);
    assert(usage.ru_idrss == 0);
    assert(usage.ru_isrss == 0);
    assert(usage.ru_nswap == 0);
    assert(usage.ru_msgsnd == 0);
    assert(usage.ru_msgrcv == 0);
    assert(usage.ru_nsignals == 0);
}

static void test_linux_zero_nsignals(void)
{
    struct sigaction action = {.sa_handler = signal_handler};
    assert(sigemptyset(&action.sa_mask) == 0);
    assert(sigaction(SIGUSR1, &action, NULL) == 0);
    struct rusage before, after;
    assert(getrusage(RUSAGE_SELF, &before) == 0);
    assert_linux_zero_legacy_rusage(before);
    assert(raise(SIGUSR1) == 0);
    assert(handled_signal == SIGUSR1);
    assert(getrusage(RUSAGE_SELF, &after) == 0);
    assert_linux_zero_legacy_rusage(after);
}

static void test_linux_zero_legacy_fields(void)
{
    struct rusage usage;
    assert(getrusage(RUSAGE_THREAD, &usage) == 0);
    assert_linux_zero_legacy_rusage(usage);
    assert(getrusage(RUSAGE_SELF, &usage) == 0);
    assert_linux_zero_legacy_rusage(usage);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0)
        _exit(0);
    int status = -1;
    assert(wait4(child, &status, 0, &usage) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert_linux_zero_legacy_rusage(usage);
    assert(getrusage(RUSAGE_CHILDREN, &usage) == 0);
    assert_linux_zero_legacy_rusage(usage);
}

static void test_self_split(void)
{
    struct rusage before, after_user, after_system;
    assert(getrusage(RUSAGE_SELF, &before) == 0);
    burn_user_time();
    assert(getrusage(RUSAGE_SELF, &after_user) == 0);
    uint64_t user_delta = timeval_us(after_user.ru_utime) - timeval_us(before.ru_utime);
    uint64_t first_system_delta =
        timeval_us(after_user.ru_stime) - timeval_us(before.ru_stime);
    assert(user_delta > 0);
    assert(user_delta != first_system_delta);

    for (size_t index = 0; index < 1000000; ++index)
        assert(syscall(SYS_gettid) > 0);
    assert(getrusage(RUSAGE_SELF, &after_system) == 0);
    uint64_t system_delta =
        timeval_us(after_system.ru_stime) - timeval_us(after_user.ru_stime);
    assert(system_delta > 0);

    long ticks_per_second = sysconf(_SC_CLK_TCK);
    assert(ticks_per_second > 0);
    struct tms tms;
    assert(times(&tms) != (clock_t)-1);
    uint64_t user_ticks = timeval_us(after_system.ru_utime) * ticks_per_second / 1000000;
    uint64_t system_ticks = timeval_us(after_system.ru_stime) * ticks_per_second / 1000000;
    assert((uint64_t)tms.tms_utime + 1 >= user_ticks &&
           (uint64_t)tms.tms_utime <= user_ticks + 1);
    assert((uint64_t)tms.tms_stime + 1 >= system_ticks &&
           (uint64_t)tms.tms_stime <= system_ticks + 1);
}

static void test_reaped_children(void)
{
    struct rusage before, after;
    assert(getrusage(RUSAGE_CHILDREN, &before) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        size_t pages = 256;
        unsigned char *mapping = mmap(NULL, pages * 4096, PROT_READ | PROT_WRITE,
                                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        assert(mapping != MAP_FAILED);
        for (size_t page = 0; page < pages; ++page)
            mapping[page * 4096] = (unsigned char)page;
        struct timespec nap = {.tv_sec = 0, .tv_nsec = 1000000};
        assert(nanosleep(&nap, NULL) == 0);
        exercise_block_io_and_major_fault();
        burn_user_time();
        _exit(0);
    }
    int status = -1;
    struct rusage child_usage;
    assert(wait4(child, &status, 0, &child_usage) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(getrusage(RUSAGE_CHILDREN, &after) == 0);
    assert(timeval_us(child_usage.ru_utime) + timeval_us(child_usage.ru_stime) > 0);
    uint64_t child_user = timeval_us(child_usage.ru_utime);
    uint64_t accumulated_user = timeval_us(after.ru_utime) - timeval_us(before.ru_utime);
    uint64_t child_system = timeval_us(child_usage.ru_stime);
    uint64_t accumulated_system = timeval_us(after.ru_stime) - timeval_us(before.ru_stime);
    assert(accumulated_user + 20000 >= child_user && child_user + 20000 >= accumulated_user);
    assert(accumulated_system + 20000 >= child_system &&
           child_system + 20000 >= accumulated_system);
    assert(child_usage.ru_minflt >= 256);
    assert(child_usage.ru_maxrss >= 1024);
    assert(child_usage.ru_nvcsw >= 1);
    assert(child_usage.ru_inblock > 0);
    assert(child_usage.ru_oublock >= 16);
    assert(after.ru_minflt - before.ru_minflt == child_usage.ru_minflt);
    assert(after.ru_majflt - before.ru_majflt == child_usage.ru_majflt);
    assert(after.ru_nvcsw - before.ru_nvcsw == child_usage.ru_nvcsw);
    assert(after.ru_nivcsw - before.ru_nivcsw == child_usage.ru_nivcsw);
    assert(after.ru_inblock - before.ru_inblock == child_usage.ru_inblock);
    assert(after.ru_oublock - before.ru_oublock == child_usage.ru_oublock);
    assert(after.ru_maxrss == (before.ru_maxrss > child_usage.ru_maxrss
                                  ? before.ru_maxrss
                                  : child_usage.ru_maxrss));
}

static void test_fault_rss_and_voluntary_switch(void)
{
    struct rusage before, after, after_unmap;
    assert(getrusage(RUSAGE_THREAD, &before) == 0);
    size_t pages = (size_t)before.ru_maxrss / 4 + 8192;
    unsigned char *mapping = mmap(NULL, pages * 4096, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    assert(mapping != MAP_FAILED);
    assert(madvise(mapping, pages * 4096, MADV_NOHUGEPAGE) == 0);
    for (size_t page = 0; page < pages; ++page)
        mapping[page * 4096] = (unsigned char)page;
    struct timespec nap = {.tv_sec = 0, .tv_nsec = 1000000};
    assert(nanosleep(&nap, NULL) == 0);
    assert(getrusage(RUSAGE_THREAD, &after) == 0);
    assert(after.ru_minflt - before.ru_minflt >= (long)pages);
    assert(after.ru_maxrss > before.ru_maxrss);
    assert(after.ru_maxrss >= (long)pages * 4);
    assert(after.ru_nvcsw > before.ru_nvcsw);
    assert(munmap(mapping, pages * 4096) == 0);
    assert(getrusage(RUSAGE_THREAD, &after_unmap) == 0);
    assert(after_unmap.ru_maxrss >= after.ru_maxrss);
}

struct thread_usage_result {
    uint64_t user_us;
    uint64_t system_us;
};

static void *thread_usage_worker(void *argument)
{
    struct thread_usage_result *result = argument;
    struct rusage before, after;
    assert(getrusage(RUSAGE_THREAD, &before) == 0);
    burn_user_time();
    assert(getrusage(RUSAGE_THREAD, &after) == 0);
    result->user_us = timeval_us(after.ru_utime) - timeval_us(before.ru_utime);
    result->system_us = timeval_us(after.ru_stime) - timeval_us(before.ru_stime);
    return NULL;
}

static void test_thread_usage(void)
{
    struct rusage self_before, self_after, main_before, main_after, invalid;
    struct thread_usage_result worker = {0};
    assert(getrusage(RUSAGE_SELF, &self_before) == 0);
    assert(getrusage(RUSAGE_THREAD, &main_before) == 0);
    pthread_t thread;
    assert(pthread_create(&thread, NULL, thread_usage_worker, &worker) == 0);
    burn_user_time();
    assert(pthread_join(thread, NULL) == 0);
    assert(getrusage(RUSAGE_THREAD, &main_after) == 0);
    assert(getrusage(RUSAGE_SELF, &self_after) == 0);
    uint64_t main_delta = timeval_us(main_after.ru_utime) + timeval_us(main_after.ru_stime) -
                          timeval_us(main_before.ru_utime) - timeval_us(main_before.ru_stime);
    uint64_t worker_delta = worker.user_us + worker.system_us;
    uint64_t self_delta = timeval_us(self_after.ru_utime) + timeval_us(self_after.ru_stime) -
                          timeval_us(self_before.ru_utime) - timeval_us(self_before.ru_stime);
    assert(main_delta > 0 && worker_delta > 0);
    assert(self_delta + 20000 >= main_delta + worker_delta);
    errno = 0;
    assert(getrusage(2, &invalid) == -1 && errno == EINVAL);
}

int main(void)
{
    test_self_split();
    test_reaped_children();
    test_thread_usage();
    test_fault_rss_and_voluntary_switch();
    exercise_block_io_and_major_fault();
    test_linux_zero_nsignals();
    test_linux_zero_legacy_fields();
    puts("CPU_ACCOUNTING_PHASE5_LINUX ALL PASS");
    return 0;
}
