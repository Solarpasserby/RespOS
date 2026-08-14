#define _GNU_SOURCE

#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/shm.h>
#include <sys/wait.h>
#include <unistd.h>

struct probe_page {
    _Atomic uint32_t child_ready;
    _Atomic uint32_t child_release;
};

static _Atomic uint32_t thread_ready;
static _Atomic uint32_t thread_release;

static unsigned long nattch(int shmid)
{
    struct shmid_ds ds;
    assert(shmctl(shmid, IPC_STAT, &ds) == 0);
    return ds.shm_nattch;
}

static void *thread_worker(void *unused)
{
    (void)unused;
    atomic_store_explicit(&thread_ready, 1, memory_order_release);
    while (atomic_load_explicit(&thread_release, memory_order_acquire) == 0)
        sched_yield();
    return NULL;
}

int main(void)
{
    int shmid = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
    assert(shmid >= 0);
    assert(nattch(shmid) == 0);

    struct probe_page *first = shmat(shmid, NULL, 0);
    assert(first != (void *)-1);
    assert(nattch(shmid) == 1);
    void *second = shmat(shmid, NULL, 0);
    assert(second != (void *)-1);
    assert(nattch(shmid) == 2);

    atomic_store_explicit(&thread_ready, 0, memory_order_relaxed);
    atomic_store_explicit(&thread_release, 0, memory_order_relaxed);
    pthread_t thread;
    assert(pthread_create(&thread, NULL, thread_worker, NULL) == 0);
    while (atomic_load_explicit(&thread_ready, memory_order_acquire) == 0)
        sched_yield();
    assert(nattch(shmid) == 2);
    atomic_store_explicit(&thread_release, 1, memory_order_release);
    assert(pthread_join(thread, NULL) == 0);
    assert(nattch(shmid) == 2);

    atomic_store_explicit(&first->child_ready, 0, memory_order_relaxed);
    atomic_store_explicit(&first->child_release, 0, memory_order_relaxed);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        atomic_store_explicit(&first->child_ready, 1, memory_order_release);
        while (atomic_load_explicit(&first->child_release,
                                    memory_order_acquire) == 0)
            sched_yield();
        _exit(0);
    }

    while (atomic_load_explicit(&first->child_ready, memory_order_acquire) == 0)
        sched_yield();
    assert(nattch(shmid) == 4);
    atomic_store_explicit(&first->child_release, 1, memory_order_release);
    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(nattch(shmid) == 2);

    assert(shmdt(second) == 0);
    assert(nattch(shmid) == 1);
    assert(shmdt(first) == 0);
    assert(nattch(shmid) == 0);
    assert(shmctl(shmid, IPC_RMID, NULL) == 0);

    puts("SYSV_SHM_NATTCH_LINUX PASS");
    return 0;
}
