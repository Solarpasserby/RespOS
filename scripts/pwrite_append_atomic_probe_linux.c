#define _GNU_SOURCE

#include <assert.h>
#include <fcntl.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
    RECORD_SIZE = 128 * 1024,
    ROUNDS = 32,
};

struct control {
    _Atomic uint32_t ready;
    _Atomic uint32_t go;
};

static int record_is(const unsigned char *buf, unsigned char value)
{
    for (size_t i = 0; i < RECORD_SIZE; ++i) {
        if (buf[i] != value)
            return 0;
    }
    return 1;
}

int main(void)
{
    char path[] = "/tmp/respos-pwrite-append-atomic-XXXXXX";
    struct control *control = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                                   MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    assert(control != MAP_FAILED);

    unsigned char *first = malloc(RECORD_SIZE);
    unsigned char *second = malloc(RECORD_SIZE);
    unsigned char *actual = malloc(2 * RECORD_SIZE);
    assert(first && second && actual);
    memset(first, 0x41, RECORD_SIZE);
    memset(second, 0x62, RECORD_SIZE);

    int fd = mkstemp(path);
    assert(fd >= 0);
    int flags = fcntl(fd, F_GETFL);
    assert(flags >= 0);
    assert(fcntl(fd, F_SETFL, flags | O_APPEND) == 0);

    for (unsigned int round = 0; round < ROUNDS; ++round) {
        assert(ftruncate(fd, 0) == 0);
        atomic_store_explicit(&control->ready, 0, memory_order_relaxed);
        atomic_store_explicit(&control->go, 0, memory_order_relaxed);

        pid_t child = fork();
        assert(child >= 0);
        if (child == 0) {
            atomic_store_explicit(&control->ready, 1, memory_order_release);
            while (atomic_load_explicit(&control->go, memory_order_acquire) == 0)
                sched_yield();
            assert(pwrite(fd, first, RECORD_SIZE, 0) == RECORD_SIZE);
            _exit(0);
        }

        while (atomic_load_explicit(&control->ready, memory_order_acquire) == 0)
            sched_yield();
        atomic_store_explicit(&control->go, 1, memory_order_release);
        assert(pwrite(fd, second, RECORD_SIZE, 0) == RECORD_SIZE);

        int status = 0;
        assert(waitpid(child, &status, 0) == child);
        assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
        assert(pread(fd, actual, 2 * RECORD_SIZE, 0) == 2 * RECORD_SIZE);
        assert((record_is(actual, 0x41) && record_is(actual + RECORD_SIZE, 0x62)) ||
               (record_is(actual, 0x62) && record_is(actual + RECORD_SIZE, 0x41)));
    }

    assert(close(fd) == 0);
    assert(unlink(path) == 0);
    assert(munmap(control, 4096) == 0);
    free(actual);
    free(second);
    free(first);
    puts("PWRITE_APPEND_ATOMIC_LINUX PASS");
    return 0;
}
