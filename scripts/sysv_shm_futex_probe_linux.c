#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <linux/futex.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/shm.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

struct probe_page {
    _Atomic uint32_t futex_word;
    _Atomic uint32_t ready;
    _Atomic uint32_t sentinel;
};

static int futex_wait(_Atomic uint32_t *word, uint32_t expected,
                      const struct timespec *timeout)
{
    return (int)syscall(SYS_futex, word, FUTEX_WAIT, expected, timeout, NULL, 0);
}

static int futex_wake(_Atomic uint32_t *word, int count)
{
    return (int)syscall(SYS_futex, word, FUTEX_WAKE, count, NULL, NULL, 0);
}

int main(void)
{
    int shmid = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
    assert(shmid >= 0);
    struct probe_page *parent = shmat(shmid, NULL, 0);
    assert(parent != (void *)-1);
    atomic_store_explicit(&parent->futex_word, 0, memory_order_release);
    atomic_store_explicit(&parent->ready, 0, memory_order_release);
    atomic_store_explicit(&parent->sentinel, 0x5a17c0de, memory_order_release);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        struct probe_page *second = shmat(shmid, NULL, 0);
        assert(second != (void *)-1);
        assert(second != parent);
        assert(atomic_load_explicit(&second->sentinel, memory_order_acquire) ==
               0x5a17c0de);
        atomic_store_explicit(&second->ready, 1, memory_order_release);
        struct timespec timeout = {.tv_sec = 2, .tv_nsec = 0};
        assert(futex_wait(&second->futex_word, 0, &timeout) == 0);
        assert(shmdt(second) == 0);
        _exit(0);
    }

    while (atomic_load_explicit(&parent->ready, memory_order_acquire) != 1)
        sched_yield();

    int woke = 0;
    for (int i = 0; i < 100000 && woke == 0; ++i) {
        woke = futex_wake(&parent->futex_word, 1);
        assert(woke >= 0);
        sched_yield();
    }
    assert(woke == 1);

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(shmdt(parent) == 0);
    assert(shmctl(shmid, IPC_RMID, NULL) == 0);
    puts("SYSV_SHM_FUTEX_LINUX PASS");
    return 0;
}
