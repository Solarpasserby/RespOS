#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/shm.h>
#include <sys/wait.h>
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

static int find_shm_index_any(int shmid)
{
    struct shminfo limits;
    int max_index = shmctl(0, IPC_INFO, (struct shmid_ds *)&limits);
    assert(max_index >= 0);
    for (int index = 0; index <= max_index; ++index) {
        struct shmid_ds ds;
        errno = 0;
        int result = shmctl(index, SHM_STAT_ANY, &ds);
        if (result == shmid)
            return index;
        assert(result >= 0 || errno == EINVAL);
    }
    assert(!"created shmid was not reachable through SHM_STAT_ANY");
    return -1;
}

static int create_keyed_segment(key_t *key, int mode)
{
    *key = (key_t)(0x53000000u ^ (unsigned int)getpid());
    for (unsigned int attempt = 0; attempt < 256; ++attempt) {
        errno = 0;
        int shmid = shmget(*key, PAGE_SIZE, IPC_CREAT | IPC_EXCL | mode);
        if (shmid >= 0)
            return shmid;
        assert(errno == EEXIST);
        *key = (key_t)((unsigned int)*key + 1);
    }
    assert(!"could not allocate a unique SysV SHM key");
    return -1;
}

static void assert_access_denials(key_t key, int shmid, int index,
                                  int check_owner_denial)
{
    int lookup_none = shmget(key, 0, 0);
    errno = 0;
    int lookup_read = shmget(key, 0, 0400);
    int lookup_read_errno = errno;
    errno = 0;
    void *mapping = shmat(shmid, NULL, SHM_RDONLY);
    int shmat_errno = errno;
    int detach_result = mapping == (void *)-1 ? 0 : shmdt(mapping);

    struct shmid_ds denied;
    errno = 0;
    int ipc_stat = shmctl(shmid, IPC_STAT, &denied);
    int ipc_stat_errno = errno;
    errno = 0;
    int shm_stat = shmctl(index, SHM_STAT, &denied);
    int shm_stat_errno = errno;
    struct shmid_ds any;
    int shm_stat_any = shmctl(index, SHM_STAT_ANY, &any);

    assert(lookup_none == shmid);
    assert(lookup_read == -1 && lookup_read_errno == EACCES);
    assert(mapping == (void *)-1 && shmat_errno == EACCES);
    assert(detach_result == 0);
    assert(ipc_stat == -1 && ipc_stat_errno == EACCES);
    assert(shm_stat == -1 && shm_stat_errno == EACCES);
    assert(shm_stat_any == shmid);
    assert(any.shm_segsz == PAGE_SIZE);

    if (check_owner_denial) {
        struct shmid_ds update = {0};
        errno = 0;
        assert(shmctl(shmid, IPC_SET, &update) == -1);
        assert(errno == EPERM);
        errno = 0;
        assert(shmctl(shmid, IPC_RMID, NULL) == -1);
        assert(errno == EPERM);
    }
}

static void verify_mode_permissions(void)
{
    key_t key;
    int shmid = create_keyed_segment(&key, 0000);
    int index = find_shm_index_any(shmid);

    if (geteuid() == 0) {
        pid_t child = fork();
        assert(child >= 0);
        if (child == 0) {
            assert(setgid(65534) == 0);
            assert(setuid(65534) == 0);
            assert_access_denials(key, shmid, index, 1);
            _exit(0);
        }
        int status = 0;
        assert(waitpid(child, &status, 0) == child);
        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            int cleanup = shmctl(shmid, IPC_RMID, NULL);
            assert(cleanup == 0 || errno == EINVAL);
            assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
        }
    } else {
        assert_access_denials(key, shmid, index, 0);
    }

    struct shmid_ds update = {0};
    update.shm_perm.uid = geteuid();
    update.shm_perm.gid = getegid();
    update.shm_perm.mode = 0000;
    int owner_set = shmctl(shmid, IPC_SET, &update);
    int owner_remove = shmctl(shmid, IPC_RMID, NULL);
    assert(owner_set == 0);
    assert(owner_remove == 0);
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

    verify_mode_permissions();

    printf("SYSV_SHM_METADATA_LINUX PASS index=%d size=%d pages=2 "
           "mode_access=pass\n",
           index, SEGMENT_SIZE);
    return 0;
}
