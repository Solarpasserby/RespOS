#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/shm.h>
#include <unistd.h>

enum {
    PAGE_SIZE = 4096,
    SEGMENT_SIZE = PAGE_SIZE + 17,
};

static struct shm_info read_shm_info(void)
{
    struct shm_info info;
    assert(shmctl(0, SHM_INFO, (struct shmid_ds *)&info) >= 0);
    return info;
}

static int find_shm_index(int shmid)
{
    struct shminfo limits;
    int max_index = shmctl(0, IPC_INFO, (struct shmid_ds *)&limits);
    assert(max_index >= 0);
    for (int index = 0; index <= max_index; ++index) {
        struct shmid_ds ds;
        errno = 0;
        int result = shmctl(index, SHM_STAT, &ds);
        if (result == shmid)
            return index;
        assert(result >= 0 || errno == EINVAL || errno == EACCES);
    }
    assert(!"created shmid was not reachable through SHM_STAT");
    return -1;
}

int main(void)
{
    struct shm_info before = read_shm_info();
    assert(before.used_ids >= 0);

    int shmid = shmget(IPC_PRIVATE, SEGMENT_SIZE, IPC_CREAT | 0640);
    assert(shmid >= 0);

    struct shmid_ds initial;
    assert(shmctl(shmid, IPC_STAT, &initial) == 0);
    assert(initial.shm_perm.__key == IPC_PRIVATE);
    assert(initial.shm_perm.uid == geteuid());
    assert(initial.shm_perm.gid == getegid());
    assert(initial.shm_perm.cuid == geteuid());
    assert(initial.shm_perm.cgid == getegid());
    assert((initial.shm_perm.mode & 0777) == 0640);
    assert(initial.shm_segsz == SEGMENT_SIZE);
    assert(initial.shm_cpid == getpid());
    assert(initial.shm_lpid == 0);
    assert(initial.shm_nattch == 0);
    assert(initial.shm_atime == 0);
    assert(initial.shm_dtime == 0);

    struct shm_info created = read_shm_info();
    assert(created.used_ids == before.used_ids + 1);
    assert(created.shm_tot == before.shm_tot + 2);

    int index = find_shm_index(shmid);
    struct shmid_ds indexed;
    assert(shmctl(index, SHM_STAT, &indexed) == shmid);
    assert(indexed.shm_segsz == SEGMENT_SIZE);
    struct shmid_ds any;
    assert(shmctl(index, SHM_STAT_ANY, &any) == shmid);
    assert(any.shm_cpid == getpid());

    uint8_t *mapping = shmat(shmid, NULL, 0);
    assert(mapping != (void *)-1);
    mapping[0] = 0x51;
    mapping[SEGMENT_SIZE - 1] = 0xa7;

    struct shmid_ds attached;
    assert(shmctl(shmid, IPC_STAT, &attached) == 0);
    assert(attached.shm_nattch == 1);
    assert(attached.shm_lpid == getpid());
    assert(attached.shm_atime >= initial.shm_ctime);

    assert(shmdt(mapping) == 0);
    struct shmid_ds detached;
    assert(shmctl(shmid, IPC_STAT, &detached) == 0);
    assert(detached.shm_nattch == 0);
    assert(detached.shm_lpid == getpid());
    assert(detached.shm_dtime >= attached.shm_atime);

    struct shmid_ds update = detached;
    update.shm_perm.mode = 0604;
    assert(shmctl(shmid, IPC_SET, &update) == 0);
    struct shmid_ds updated;
    assert(shmctl(shmid, IPC_STAT, &updated) == 0);
    assert((updated.shm_perm.mode & 0777) == 0604);
    assert(updated.shm_ctime >= detached.shm_ctime);

    mapping = shmat(shmid, NULL, 0);
    assert(mapping != (void *)-1);
    assert(mapping[0] == 0x51);
    assert(mapping[SEGMENT_SIZE - 1] == 0xa7);
    assert(shmctl(shmid, IPC_RMID, NULL) == 0);

    struct shmid_ds removed;
    assert(shmctl(shmid, IPC_STAT, &removed) == 0);
    assert((removed.shm_perm.mode & SHM_DEST) != 0);
    assert(removed.shm_nattch == 1);
    struct shm_info pending = read_shm_info();
    assert(pending.used_ids == before.used_ids + 1);
    assert(pending.shm_tot == before.shm_tot + 2);

    assert(shmdt(mapping) == 0);
    errno = 0;
    assert(shmctl(shmid, IPC_STAT, &removed) == -1);
    assert(errno == EINVAL);
    struct shm_info after = read_shm_info();
    assert(after.used_ids == before.used_ids);
    assert(after.shm_tot == before.shm_tot);

    printf("SYSV_SHM_METADATA_LINUX PASS index=%d size=%d pages=2\n", index,
           SEGMENT_SIZE);
    return 0;
}
