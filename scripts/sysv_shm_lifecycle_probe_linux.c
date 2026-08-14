#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/shm.h>
#include <sys/wait.h>
#include <unistd.h>

static key_t probe_key(unsigned int slot)
{
    return (key_t)(0x52000000u | (((unsigned int)getpid() & 0xffffu) << 4) |
                   (slot & 0xfu));
}

static int create_segment(key_t key)
{
    int shmid = shmget(key, 4096, IPC_CREAT | IPC_EXCL | 0600);
    assert(shmid >= 0);
    return shmid;
}

static void expect_stale_id(int shmid)
{
    errno = 0;
    assert(shmat(shmid, NULL, 0) == (void *)-1);
    assert(errno == EINVAL);
}

static void test_explicit_last_detach(void)
{
    key_t key = probe_key(1);
    int old_id = create_segment(key);
    uint32_t *first = shmat(old_id, NULL, 0);
    assert(first != (void *)-1);
    *first = 0x51a7c0deu;

    assert(shmctl(old_id, IPC_RMID, NULL) == 0);
    errno = 0;
    assert(shmget(key, 4096, 0) == -1);
    assert(errno == ENOENT);

    int replacement = create_segment(key);
    assert(replacement != old_id);

    uint32_t *second = shmat(old_id, NULL, 0);
    assert(second != (void *)-1);
    assert(*second == 0x51a7c0deu);
    assert(shmdt(second) == 0);
    assert(*first == 0x51a7c0deu);
    assert(shmdt(first) == 0);
    expect_stale_id(old_id);

    assert(shmctl(replacement, IPC_RMID, NULL) == 0);
}

static void test_exit_without_shmdt(void)
{
    key_t key = probe_key(2);
    int shmid = create_segment(key);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        uint32_t *mapping = shmat(shmid, NULL, 0);
        assert(mapping != (void *)-1);
        *mapping = 0xe817c0deu;
        assert(shmctl(shmid, IPC_RMID, NULL) == 0);
        _exit(0);
    }

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    expect_stale_id(shmid);

    int replacement = create_segment(key);
    assert(shmctl(replacement, IPC_RMID, NULL) == 0);
}

static void test_exec_without_shmdt(void)
{
    key_t key = probe_key(3);
    int shmid = create_segment(key);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        uint32_t *mapping = shmat(shmid, NULL, 0);
        assert(mapping != (void *)-1);
        *mapping = 0xec7ec0deu;
        assert(shmctl(shmid, IPC_RMID, NULL) == 0);
        execl("/bin/true", "true", NULL);
        _exit(127);
    }

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    expect_stale_id(shmid);

    int replacement = create_segment(key);
    assert(shmctl(replacement, IPC_RMID, NULL) == 0);
}

static void test_fork_inherited_attachment(void)
{
    key_t key = probe_key(4);
    int shmid = create_segment(key);
    uint32_t *parent = shmat(shmid, NULL, 0);
    assert(parent != (void *)-1);
    *parent = 0xf04cc0deu;
    assert(shmctl(shmid, IPC_RMID, NULL) == 0);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(*parent == 0xf04cc0deu);
        _exit(0);
    }

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(*parent == 0xf04cc0deu);

    uint32_t *second = shmat(shmid, NULL, 0);
    assert(second != (void *)-1);
    assert(*second == 0xf04cc0deu);
    assert(shmdt(second) == 0);
    assert(shmdt(parent) == 0);
    expect_stale_id(shmid);
}

static void test_signal_exit_without_shmdt(void)
{
    key_t key = probe_key(5);
    int shmid = create_segment(key);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        uint32_t *mapping = shmat(shmid, NULL, 0);
        assert(mapping != (void *)-1);
        *mapping = 0x519ac0deu;
        assert(shmctl(shmid, IPC_RMID, NULL) == 0);
        assert(kill(getpid(), SIGKILL) == 0);
        _exit(127);
    }

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
    expect_stale_id(shmid);

    int replacement = create_segment(key);
    assert(shmctl(replacement, IPC_RMID, NULL) == 0);
}

int main(void)
{
    test_explicit_last_detach();
    test_exit_without_shmdt();
    test_exec_without_shmdt();
    test_fork_inherited_attachment();
    test_signal_exit_without_shmdt();
    puts("SYSV_SHM_LIFECYCLE_LINUX PASS");
    return 0;
}
