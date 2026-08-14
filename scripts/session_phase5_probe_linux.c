#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

static void wait_child(pid_t child)
{
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

static void test_getsid_query_and_errors(void)
{
    pid_t self = getpid();
    pid_t sid = getsid(0);
    assert(sid > 0);
    assert(getsid(self) == sid);

    errno = 0;
    assert(getsid(-1) == -1);
    assert(errno == ESRCH);

    errno = 0;
    assert(getsid(1 << 29) == -1);
    assert(errno == ESRCH);
    puts("SESSION_PHASE5_LINUX query_errors PASS");
}

static void test_child_setsid_and_parent_query(void)
{
    int ready[2];
    int release[2];
    assert(pipe(ready) == 0);
    assert(pipe(release) == 0);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(close(ready[0]) == 0);
        assert(close(release[1]) == 0);
        pid_t sid = setsid();
        assert(sid == getpid());
        assert(getsid(0) == sid);
        assert(write(ready[1], &sid, sizeof(sid)) == (ssize_t)sizeof(sid));
        char byte = 0;
        assert(read(release[0], &byte, 1) == 1);
        _exit(0);
    }

    assert(close(ready[1]) == 0);
    assert(close(release[0]) == 0);
    pid_t child_sid = -1;
    assert(read(ready[0], &child_sid, sizeof(child_sid)) == (ssize_t)sizeof(child_sid));
    assert(child_sid == child);
    assert(getsid(child) == child_sid);
    assert(write(release[1], "x", 1) == 1);
    wait_child(child);
    assert(close(ready[0]) == 0);
    assert(close(release[1]) == 0);
    puts("SESSION_PHASE5_LINUX child_setsid_parent_query PASS");
}

static void test_process_group_leader_cannot_setsid(void)
{
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(setpgid(0, 0) == 0);
        assert(getpgid(0) == getpid());
        errno = 0;
        assert(setsid() == -1);
        assert(errno == EPERM);
        _exit(0);
    }
    wait_child(child);
    puts("SESSION_PHASE5_LINUX pgrp_leader_setsid_eperm PASS");
}

int main(void)
{
    setbuf(stdout, NULL);
    test_getsid_query_and_errors();
    test_child_setsid_and_parent_query();
    test_process_group_leader_cannot_setsid();
    puts("SESSION_PHASE5_LINUX ALL PASS");
    return 0;
}
