#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
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

static unsigned long verify_shmmni_limit(void)
{
    struct shminfo limits;
    struct shm_info before;
    assert(shmctl(0, IPC_INFO, (struct shmid_ds *)&limits) >= 0);
    assert(shmctl(0, SHM_INFO, (struct shmid_ds *)&before) >= 0);
    assert(before.used_ids >= 0);
    assert(limits.shmmni > (unsigned long)before.used_ids);

    size_t available = limits.shmmni - (unsigned long)before.used_ids;
    int *ids = calloc(available + 1, sizeof(*ids));
    assert(ids != NULL);
    size_t created = 0;
    for (; created < available; ++created) {
        ids[created] = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
        if (ids[created] < 0)
            break;
    }
    if (created != available) {
        for (size_t idx = 0; idx < created; ++idx)
            assert(shmctl(ids[idx], IPC_RMID, NULL) == 0);
        free(ids);
        assert(created == available);
    }

    errno = 0;
    int overflow = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
    if (overflow >= 0) {
        assert(shmctl(overflow, IPC_RMID, NULL) == 0);
        for (size_t idx = 0; idx < created; ++idx)
            assert(shmctl(ids[idx], IPC_RMID, NULL) == 0);
        free(ids);
        assert(overflow < 0);
    }
    int overflow_errno = errno;
    if (overflow_errno != ENOSPC) {
        for (size_t idx = 0; idx < created; ++idx)
            assert(shmctl(ids[idx], IPC_RMID, NULL) == 0);
        free(ids);
        assert(overflow_errno == ENOSPC);
    }

    assert(shmctl(ids[created - 1], IPC_RMID, NULL) == 0);
    --created;
    int replacement = shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0600);
    if (replacement < 0) {
        for (size_t idx = 0; idx < created; ++idx)
            assert(shmctl(ids[idx], IPC_RMID, NULL) == 0);
        free(ids);
        assert(replacement >= 0);
    }
    ids[created++] = replacement;

    for (size_t idx = 0; idx < created; ++idx)
        assert(shmctl(ids[idx], IPC_RMID, NULL) == 0);
    free(ids);

    struct shm_info after;
    assert(shmctl(0, SHM_INFO, (struct shmid_ds *)&after) >= 0);
    assert(after.used_ids == before.used_ids);
    return limits.shmmni;
}

struct race_control {
    _Atomic uint32_t ready;
    _Atomic uint32_t go;
};

int main(void)
{
    unsigned long shmmni = verify_shmmni_limit();

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
    printf("SYSV_SHM_ATTACH_RACE_LINUX PASS shmmni=%lu pressure=%u "
           "attempts=%u invalid=%u attached=%u\n",
           shmmni, PRESSURE_ROUNDS, ROUNDS * ATTACHERS, invalid, attached);
    return 0;
}
