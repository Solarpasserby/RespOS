#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/shm.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
    CHILD_INVALID = 10,
    CHILD_ATTACHED = 11,
    CHILD_ORPHAN = 12,
    ATTACHERS = 2,
    ROUNDS = 32,
    PRESSURE_ROUNDS = 128,
};

static int removed_errno(int value)
{
    return value == EINVAL || value == EIDRM;
}

struct race_control {
    _Atomic uint32_t ready;
    _Atomic uint32_t go;
};

int main(void)
{
    struct race_control *control =
        mmap(NULL, 4096, PROT_READ | PROT_WRITE,
             MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    assert(control != MAP_FAILED);

    int rollback_id = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
    assert(rollback_id >= 0);
    void *rollback_survivor = shmat(rollback_id, NULL, 0);
    assert(rollback_survivor != (void *)-1);
    assert(shmctl(rollback_id, IPC_RMID, NULL) == 0);
    errno = 0;
    assert(shmat(rollback_id, control, 0) == (void *)-1);
    assert(errno == EINVAL);
    assert(shmdt(rollback_survivor) == 0);
    errno = 0;
    assert(shmat(rollback_id, NULL, 0) == (void *)-1);
    assert(errno == EINVAL);

    for (unsigned int round = 0; round < PRESSURE_ROUNDS; ++round) {
        int pressure_id = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
        assert(pressure_id >= 0);
        uint32_t *pressure = shmat(pressure_id, NULL, 0);
        assert(pressure != (void *)-1);
        *pressure = round ^ 0x51a7u;
        assert(shmctl(pressure_id, IPC_RMID, NULL) == 0);
        assert(*pressure == (round ^ 0x51a7u));
        assert(shmdt(pressure) == 0);
        errno = 0;
        assert(shmat(pressure_id, NULL, 0) == (void *)-1);
        assert(errno == EINVAL);
    }

    unsigned int invalid = 0;
    unsigned int attached = 0;
    for (unsigned int round = 0; round < ROUNDS; ++round) {
        atomic_store_explicit(&control->ready, 0, memory_order_relaxed);
        atomic_store_explicit(&control->go, 0, memory_order_relaxed);

        int shmid = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
        assert(shmid >= 0);
        uint32_t *parent = shmat(shmid, NULL, 0);
        assert(parent != (void *)-1);
        *parent = 0xa77ac0deu;
        assert(shmctl(shmid, IPC_RMID, NULL) == 0);

        pid_t children[ATTACHERS];
        for (unsigned int idx = 0; idx < ATTACHERS; ++idx) {
            pid_t child = fork();
            assert(child >= 0);
            if (child == 0) {
                assert(shmdt(parent) == 0);
                atomic_fetch_add_explicit(&control->ready, 1,
                                          memory_order_release);
                while (atomic_load_explicit(&control->go,
                                            memory_order_acquire) == 0)
                    sched_yield();

                errno = 0;
                uint32_t *mapping = shmat(shmid, NULL, 0);
                if (mapping == (void *)-1) {
                    assert(removed_errno(errno));
                    _exit(CHILD_INVALID);
                }

                assert(*mapping == 0xa77ac0deu);
                struct shmid_ds ds;
                if (shmctl(shmid, IPC_STAT, &ds) != 0) {
                    assert(removed_errno(errno));
                    assert(shmdt(mapping) == 0);
                    _exit(CHILD_ORPHAN);
                }
                assert(ds.shm_nattch >= 1);
                assert(shmdt(mapping) == 0);
                _exit(CHILD_ATTACHED);
            }
            children[idx] = child;
        }

        while (atomic_load_explicit(&control->ready,
                                    memory_order_acquire) != ATTACHERS)
            sched_yield();
        atomic_store_explicit(&control->go, 1, memory_order_release);
        sched_yield();
        sched_yield();
        assert(shmdt(parent) == 0);

        for (unsigned int idx = 0; idx < ATTACHERS; ++idx) {
            int status = 0;
            assert(waitpid(children[idx], &status, 0) == children[idx]);
            assert(WIFEXITED(status));
            int code = WEXITSTATUS(status);
            assert(code != CHILD_ORPHAN);
            if (code == CHILD_INVALID)
                ++invalid;
            else {
                assert(code == CHILD_ATTACHED);
                ++attached;
            }
        }

        errno = 0;
        assert(shmat(shmid, NULL, 0) == (void *)-1);
        assert(errno == EINVAL);
    }

    assert(munmap(control, 4096) == 0);
    assert(invalid + attached == ROUNDS * ATTACHERS);
    printf("SYSV_SHM_ATTACH_RACE_LINUX PASS pressure=%u attempts=%u "
           "invalid=%u attached=%u\n",
           PRESSURE_ROUNDS, ROUNDS * ATTACHERS, invalid, attached);
    return 0;
}
