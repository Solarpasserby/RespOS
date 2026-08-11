#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <sched.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
    STACK_SIZE = 64 * 1024,
    EXEC_FAILURE = 111,
    EXEC_TARGET_STATUS = 23,
    LEADER_EXIT_STATUS = 42,
    WORKER_EXIT_STATUS = 7,
};

struct leader_exit_state {
    atomic_int worker_ready;
    atomic_int leader_may_exit;
    atomic_int worker_survived;
};

static int leader_exit_worker(void *opaque)
{
    struct leader_exit_state *state = opaque;

    atomic_store_explicit(&state->worker_ready, 1, memory_order_release);
    while (atomic_load_explicit(&state->leader_may_exit, memory_order_acquire) == 0)
        sched_yield();

    /* Give the leader ample time to complete SYS_exit. */
    usleep(100000);
    atomic_store_explicit(&state->worker_survived, 1, memory_order_release);
    syscall(SYS_exit_group, WORKER_EXIT_STATUS);
    __builtin_unreachable();
}

static int leader_exit_worker_raw(void *opaque)
{
    struct leader_exit_state *state = opaque;

    atomic_store_explicit(&state->worker_ready, 1, memory_order_release);
    while (atomic_load_explicit(&state->leader_may_exit, memory_order_acquire) == 0)
        sched_yield();

    usleep(100000);
    atomic_store_explicit(&state->worker_survived, 1, memory_order_release);
    syscall(SYS_exit, WORKER_EXIT_STATUS);
    __builtin_unreachable();
}

static void test_leader_sys_exit_case(int (*worker)(void *), int expected_status,
                                      const char *label)
{
    struct leader_exit_state *state = mmap(NULL, sizeof(*state), PROT_READ | PROT_WRITE,
                                            MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    assert(state != MAP_FAILED);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        void *stack = mmap(NULL, STACK_SIZE, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
        if (stack == MAP_FAILED)
            syscall(SYS_exit_group, 120);

        int flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
        if (clone(worker, (char *)stack + STACK_SIZE, flags, state) < 0)
            syscall(SYS_exit_group, 121);

        while (atomic_load_explicit(&state->worker_ready, memory_order_acquire) == 0)
            sched_yield();
        atomic_store_explicit(&state->leader_may_exit, 1, memory_order_release);
        syscall(SYS_exit, LEADER_EXIT_STATUS);
        __builtin_unreachable();
    }

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status));
    assert(WEXITSTATUS(status) == expected_status);
    assert(atomic_load_explicit(&state->worker_survived, memory_order_acquire) == 1);
    assert(munmap(state, sizeof(*state)) == 0);
    printf("TASK_PHASE5_LINUX %s PASS\n", label);
}

struct exec_state {
    const char *self;
};

static int nonleader_exec_worker(void *opaque)
{
    const struct exec_state *state = opaque;
    char *const argv[] = {(char *)state->self, (char *)"--exec-target", NULL};

    execv(state->self, argv);
    syscall(SYS_exit_group, EXEC_FAILURE);
    __builtin_unreachable();
}

static void test_nonleader_exec(const char *self)
{
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        void *stack = mmap(NULL, STACK_SIZE, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
        if (stack == MAP_FAILED)
            syscall(SYS_exit_group, 122);

        struct exec_state state = {.self = self};
        int flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
        if (clone(nonleader_exec_worker, (char *)stack + STACK_SIZE, flags, &state) < 0)
            syscall(SYS_exit_group, 123);

        for (;;)
            pause();
    }

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status));
    assert(WEXITSTATUS(status) == EXEC_TARGET_STATUS);
    puts("TASK_PHASE5_LINUX nonleader_exec PASS");
}

int main(int argc, char **argv)
{
    if (argc == 2 && strcmp(argv[1], "--exec-target") == 0) {
        assert(getpid() == (pid_t)syscall(SYS_gettid));
        return EXEC_TARGET_STATUS;
    }

    assert(argc >= 1);
    test_leader_sys_exit_case(leader_exit_worker, WORKER_EXIT_STATUS,
                              "leader_exit_then_exit_group");
    test_leader_sys_exit_case(leader_exit_worker_raw, WORKER_EXIT_STATUS,
                              "leader_exit_then_worker_exit");
    test_nonleader_exec(argv[0]);
    puts("TASK_PHASE5_LINUX ALL PASS");
    return 0;
}
