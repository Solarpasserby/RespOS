#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <spawn.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

#define CHECK(condition, message)                                                \
    do {                                                                         \
        if (!(condition)) {                                                       \
            fprintf(stderr, "LIBC_COMBINATION_FAIL %s line=%d errno=%d\n",       \
                    (message), __LINE__, errno);                                  \
            return 1;                                                            \
        }                                                                        \
    } while (0)

enum { THREAD_COUNT = 4, RESOURCE_ROUNDS = 32 };

struct thread_state {
    pthread_mutex_t mutex;
    pthread_cond_t condition;
    pthread_rwlock_t rwlock;
    int ready;
    int release;
    int value;
};

static pthread_once_t once_control = PTHREAD_ONCE_INIT;
static pthread_key_t tls_key;
static atomic_int once_count;
static atomic_int destructor_count;

static void once_initializer(void) {
    atomic_fetch_add_explicit(&once_count, 1, memory_order_relaxed);
}

static void tls_destructor(void *value) {
    if (value != NULL) {
        atomic_fetch_add_explicit(&destructor_count, 1, memory_order_relaxed);
    }
}

static void *joined_worker(void *argument) {
    struct thread_state *state = argument;
    if (pthread_once(&once_control, once_initializer) != 0 ||
        pthread_setspecific(tls_key, state) != 0 ||
        pthread_mutex_lock(&state->mutex) != 0) {
        return (void *)1;
    }
    state->ready++;
    pthread_cond_broadcast(&state->condition);
    while (!state->release) {
        if (pthread_cond_wait(&state->condition, &state->mutex) != 0) {
            pthread_mutex_unlock(&state->mutex);
            return (void *)2;
        }
    }
    state->value++;
    pthread_mutex_unlock(&state->mutex);
    return NULL;
}

struct detached_state {
    pthread_mutex_t mutex;
    pthread_cond_t condition;
    int ready;
    int release;
    int finished;
};

static void *detached_worker(void *argument) {
    struct detached_state *state = argument;
    if (pthread_mutex_lock(&state->mutex) != 0) {
        return NULL;
    }
    state->ready = 1;
    pthread_cond_signal(&state->condition);
    while (!state->release) {
        if (pthread_cond_wait(&state->condition, &state->mutex) != 0) {
            pthread_mutex_unlock(&state->mutex);
            return NULL;
        }
    }
    state->finished = 1;
    pthread_cond_signal(&state->condition);
    pthread_mutex_unlock(&state->mutex);
    return NULL;
}

static void *reusable_worker(void *argument) {
    return argument;
}

static void *robust_owner(void *argument) {
    pthread_mutex_t *mutex = argument;
    if (pthread_mutex_lock(mutex) != 0) {
        return (void *)1;
    }
    return NULL;
}

struct shared_state {
    pthread_mutex_t mutex;
    pthread_cond_t condition;
    int ready;
    int release;
};

static int wait_child(pid_t pid, int expected) {
    int status = 0;
    return waitpid(pid, &status, 0) == pid && WIFEXITED(status) &&
           WEXITSTATUS(status) == expected;
}

static int test_pthread_core(void) {
    struct thread_state state;
    pthread_t threads[THREAD_COUNT];
    memset(&state, 0, sizeof(state));
    CHECK(pthread_mutex_init(&state.mutex, NULL) == 0, "mutex_init");
    CHECK(pthread_cond_init(&state.condition, NULL) == 0, "cond_init");
    CHECK(pthread_rwlock_init(&state.rwlock, NULL) == 0, "rwlock_init");
    CHECK(pthread_key_create(&tls_key, tls_destructor) == 0, "key_create");

    for (int index = 0; index < THREAD_COUNT; ++index) {
        CHECK(pthread_create(&threads[index], NULL, joined_worker, &state) == 0,
              "pthread_create");
    }
    CHECK(pthread_mutex_lock(&state.mutex) == 0, "main_mutex_lock");
    while (state.ready != THREAD_COUNT) {
        CHECK(pthread_cond_wait(&state.condition, &state.mutex) == 0,
              "main_cond_wait");
    }
    state.release = 1;
    CHECK(pthread_cond_broadcast(&state.condition) == 0, "main_cond_broadcast");
    CHECK(pthread_mutex_unlock(&state.mutex) == 0, "main_mutex_unlock");

    for (int index = 0; index < THREAD_COUNT; ++index) {
        void *result = (void *)1;
        CHECK(pthread_join(threads[index], &result) == 0 && result == NULL,
              "pthread_join");
    }
    CHECK(state.value == THREAD_COUNT, "joined_value");
    CHECK(atomic_load_explicit(&once_count, memory_order_relaxed) == 1,
          "pthread_once");
    CHECK(atomic_load_explicit(&destructor_count, memory_order_relaxed) ==
              THREAD_COUNT,
          "tls_destructor");

    CHECK(pthread_rwlock_wrlock(&state.rwlock) == 0, "rwlock_wrlock");
    state.value = 41;
    CHECK(pthread_rwlock_unlock(&state.rwlock) == 0, "rwlock_wrunlock");
    CHECK(pthread_rwlock_rdlock(&state.rwlock) == 0, "rwlock_rdlock");
    CHECK(state.value == 41, "rwlock_value");
    CHECK(pthread_rwlock_unlock(&state.rwlock) == 0, "rwlock_rdunlock");

    CHECK(pthread_key_delete(tls_key) == 0, "key_delete");
    CHECK(pthread_rwlock_destroy(&state.rwlock) == 0, "rwlock_destroy");
    CHECK(pthread_cond_destroy(&state.condition) == 0, "cond_destroy");
    CHECK(pthread_mutex_destroy(&state.mutex) == 0, "mutex_destroy");
    puts("LIBC_COMBINATION pthread_core PASS");
    return 0;
}

static int test_pthread_detach_and_reuse(void) {
    pthread_attr_t attributes;
    pthread_t thread;
    CHECK(pthread_attr_init(&attributes) == 0, "detach_attr_init");
    CHECK(pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED) == 0,
          "detach_attr_set");

    for (int round = 0; round < RESOURCE_ROUNDS; ++round) {
        struct detached_state detached;
        memset(&detached, 0, sizeof(detached));
        CHECK(pthread_mutex_init(&detached.mutex, NULL) == 0,
              "detach_mutex_init");
        CHECK(pthread_cond_init(&detached.condition, NULL) == 0,
              "detach_cond_init");
        CHECK(pthread_create(&thread, &attributes, detached_worker, &detached) == 0,
              "detach_create");
        CHECK(pthread_mutex_lock(&detached.mutex) == 0, "detach_mutex_lock");
        while (!detached.ready) {
            CHECK(pthread_cond_wait(&detached.condition, &detached.mutex) == 0,
                  "detach_cond_wait");
        }
        detached.release = 1;
        CHECK(pthread_cond_signal(&detached.condition) == 0,
              "detach_release_signal");
        while (!detached.finished) {
            CHECK(pthread_cond_wait(&detached.condition, &detached.mutex) == 0,
                  "detach_finish_wait");
        }
        CHECK(pthread_mutex_unlock(&detached.mutex) == 0,
              "detach_release_unlock");
        CHECK(pthread_cond_destroy(&detached.condition) == 0,
              "detach_cond_destroy");
        CHECK(pthread_mutex_destroy(&detached.mutex) == 0,
              "detach_mutex_destroy");
    }
    CHECK(pthread_attr_destroy(&attributes) == 0, "detach_attr_destroy");

    for (int round = 0; round < RESOURCE_ROUNDS; ++round) {
        void *expected = (void *)(long)(round + 1);
        void *result = NULL;
        CHECK(pthread_create(&thread, NULL, reusable_worker, expected) == 0,
              "reuse_create");
        CHECK(pthread_join(thread, &result) == 0 && result == expected,
              "reuse_join");
    }
    puts("LIBC_COMBINATION pthread_detach_reuse PASS");
    return 0;
}

static int test_pthread_robust(void) {
    pthread_mutexattr_t attributes;
    pthread_mutex_t mutex;
    pthread_t owner;
    void *result = NULL;
    CHECK(pthread_mutexattr_init(&attributes) == 0, "robust_attr_init");
    CHECK(pthread_mutexattr_setrobust(&attributes, PTHREAD_MUTEX_ROBUST) == 0,
          "robust_attr_set");
    CHECK(pthread_mutex_init(&mutex, &attributes) == 0, "robust_mutex_init");
    CHECK(pthread_mutexattr_destroy(&attributes) == 0, "robust_attr_destroy");
    CHECK(pthread_create(&owner, NULL, robust_owner, &mutex) == 0,
          "robust_owner_create");
    CHECK(pthread_join(owner, &result) == 0 && result == NULL,
          "robust_owner_join");
    CHECK(pthread_mutex_lock(&mutex) == EOWNERDEAD, "robust_owner_dead");
    CHECK(pthread_mutex_consistent(&mutex) == 0, "robust_consistent");
    CHECK(pthread_mutex_unlock(&mutex) == 0, "robust_unlock");
    CHECK(pthread_mutex_lock(&mutex) == 0, "robust_relock");
    CHECK(pthread_mutex_unlock(&mutex) == 0, "robust_reunlock");
    CHECK(pthread_mutex_destroy(&mutex) == 0, "robust_destroy");
    puts("LIBC_COMBINATION pthread_robust PASS");
    return 0;
}

static int test_pthread_pshared(void) {
    struct shared_state *state = mmap(NULL, sizeof(*state), PROT_READ | PROT_WRITE,
                                      MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    pthread_mutexattr_t mutex_attributes;
    pthread_condattr_t condition_attributes;
    CHECK(state != MAP_FAILED, "pshared_mmap");
    memset(state, 0, sizeof(*state));
    CHECK(pthread_mutexattr_init(&mutex_attributes) == 0, "pshared_mutex_attr");
    CHECK(pthread_mutexattr_setpshared(&mutex_attributes, PTHREAD_PROCESS_SHARED) == 0,
          "pshared_mutex_set");
    CHECK(pthread_condattr_init(&condition_attributes) == 0, "pshared_cond_attr");
    CHECK(pthread_condattr_setpshared(&condition_attributes, PTHREAD_PROCESS_SHARED) == 0,
          "pshared_cond_set");
    CHECK(pthread_mutex_init(&state->mutex, &mutex_attributes) == 0,
          "pshared_mutex_init");
    CHECK(pthread_cond_init(&state->condition, &condition_attributes) == 0,
          "pshared_cond_init");
    CHECK(pthread_mutexattr_destroy(&mutex_attributes) == 0,
          "pshared_mutex_attr_destroy");
    CHECK(pthread_condattr_destroy(&condition_attributes) == 0,
          "pshared_cond_attr_destroy");

    pid_t child = fork();
    CHECK(child >= 0, "pshared_fork");
    if (child == 0) {
        if (pthread_mutex_lock(&state->mutex) != 0) _exit(20);
        state->ready = 1;
        if (pthread_cond_signal(&state->condition) != 0) _exit(21);
        while (!state->release) {
            if (pthread_cond_wait(&state->condition, &state->mutex) != 0) _exit(22);
        }
        if (pthread_mutex_unlock(&state->mutex) != 0) _exit(23);
        _exit(0);
    }
    CHECK(pthread_mutex_lock(&state->mutex) == 0, "pshared_parent_lock");
    while (!state->ready) {
        CHECK(pthread_cond_wait(&state->condition, &state->mutex) == 0,
              "pshared_parent_wait");
    }
    state->release = 1;
    CHECK(pthread_cond_signal(&state->condition) == 0, "pshared_parent_signal");
    CHECK(pthread_mutex_unlock(&state->mutex) == 0, "pshared_parent_unlock");
    CHECK(wait_child(child, 0), "pshared_child_status");
    CHECK(pthread_cond_destroy(&state->condition) == 0, "pshared_cond_destroy");
    CHECK(pthread_mutex_destroy(&state->mutex) == 0, "pshared_mutex_destroy");
    CHECK(munmap(state, sizeof(*state)) == 0, "pshared_munmap");
    puts("LIBC_COMBINATION pthread_pshared PASS");
    return 0;
}

static int test_pthread_fork_exec(const char *self_path) {
    struct detached_state state;
    pthread_t sibling;
    void *result = (void *)1;
    memset(&state, 0, sizeof(state));
    CHECK(pthread_mutex_init(&state.mutex, NULL) == 0, "fork_mutex_init");
    CHECK(pthread_cond_init(&state.condition, NULL) == 0, "fork_cond_init");
    CHECK(pthread_create(&sibling, NULL, detached_worker, &state) == 0,
          "fork_sibling_create");
    CHECK(pthread_mutex_lock(&state.mutex) == 0, "fork_parent_lock");
    while (!state.ready) {
        CHECK(pthread_cond_wait(&state.condition, &state.mutex) == 0,
              "fork_parent_ready_wait");
    }
    CHECK(pthread_mutex_unlock(&state.mutex) == 0, "fork_parent_unlock");

    pid_t child = fork();
    CHECK(child >= 0, "multithread_fork");
    if (child == 0) {
        execl(self_path, self_path, "--exit-42", (char *)NULL);
        _exit(41);
    }

    CHECK(pthread_mutex_lock(&state.mutex) == 0, "fork_release_lock");
    state.release = 1;
    CHECK(pthread_cond_signal(&state.condition) == 0, "fork_release_signal");
    CHECK(pthread_mutex_unlock(&state.mutex) == 0, "fork_release_unlock");
    CHECK(pthread_join(sibling, &result) == 0 && result == NULL,
          "fork_sibling_join");
    CHECK(wait_child(child, 42), "fork_exec_child_status");
    CHECK(pthread_cond_destroy(&state.condition) == 0, "fork_cond_destroy");
    CHECK(pthread_mutex_destroy(&state.mutex) == 0, "fork_mutex_destroy");
    puts("LIBC_COMBINATION pthread_fork_exec PASS");
    return 0;
}

static int spawn_child(int closed_fd) {
    sigset_t mask;
    struct sigaction action;
    if (sigprocmask(SIG_SETMASK, NULL, &mask) != 0 ||
        sigismember(&mask, SIGUSR2) != 1) {
        return 31;
    }
    if (sigaction(SIGUSR1, NULL, &action) != 0 || action.sa_handler != SIG_DFL) {
        return 32;
    }
    if (getpgrp() != getpid()) {
        return 33;
    }
    errno = 0;
    if (fcntl(closed_fd, F_GETFD) != -1 || errno != EBADF) {
        return 34;
    }
    puts("LIBC_COMBINATION SPAWN_CHILD_OK");
    return 37;
}

static int test_posix_spawn(const char *self_path) {
    char output[] = "/tmp/respos-posix-spawn-output-XXXXXX";
    int output_fd = mkstemp(output);
    int probe_pipe[2];
    posix_spawn_file_actions_t actions;
    posix_spawnattr_t attributes;
    sigset_t mask;
    sigset_t defaults;
    struct sigaction ignore;
    pid_t child = -1;
    char closed_fd[32];
    CHECK(output_fd >= 0, "spawn_mkstemp");
    CHECK(close(output_fd) == 0, "spawn_temp_close");
    CHECK(pipe(probe_pipe) == 0, "spawn_pipe");
    CHECK(posix_spawn_file_actions_init(&actions) == 0, "spawn_actions_init");
    CHECK(posix_spawn_file_actions_addopen(&actions, STDOUT_FILENO, output,
                                           O_WRONLY | O_TRUNC, 0600) == 0,
          "spawn_addopen");
    CHECK(posix_spawn_file_actions_addclose(&actions, probe_pipe[0]) == 0,
          "spawn_addclose");
    CHECK(posix_spawnattr_init(&attributes) == 0, "spawn_attr_init");
    CHECK(sigemptyset(&mask) == 0 && sigaddset(&mask, SIGUSR2) == 0,
          "spawn_mask_build");
    CHECK(sigemptyset(&defaults) == 0 && sigaddset(&defaults, SIGUSR1) == 0,
          "spawn_default_build");
    CHECK(posix_spawnattr_setsigmask(&attributes, &mask) == 0,
          "spawn_setsigmask");
    CHECK(posix_spawnattr_setsigdefault(&attributes, &defaults) == 0,
          "spawn_setsigdefault");
    CHECK(posix_spawnattr_setpgroup(&attributes, 0) == 0, "spawn_setpgroup");
    CHECK(posix_spawnattr_setflags(&attributes,
                                   POSIX_SPAWN_SETSIGMASK |
                                       POSIX_SPAWN_SETSIGDEF |
                                       POSIX_SPAWN_SETPGROUP) == 0,
          "spawn_setflags");
    memset(&ignore, 0, sizeof(ignore));
    ignore.sa_handler = SIG_IGN;
    CHECK(sigemptyset(&ignore.sa_mask) == 0, "spawn_ignore_mask");
    CHECK(sigaction(SIGUSR1, &ignore, NULL) == 0, "spawn_parent_ignore");
    snprintf(closed_fd, sizeof(closed_fd), "%d", probe_pipe[0]);
    char *const child_argv[] = {(char *)self_path, "--spawn-child", closed_fd, NULL};
    int spawn_result = posix_spawn(&child, self_path, &actions, &attributes,
                                   child_argv, environ);
    CHECK(spawn_result == 0, "posix_spawn");
    CHECK(wait_child(child, 37), "spawn_child_status");
    CHECK(posix_spawn_file_actions_destroy(&actions) == 0,
          "spawn_actions_destroy");
    CHECK(posix_spawnattr_destroy(&attributes) == 0, "spawn_attr_destroy");
    CHECK(close(probe_pipe[0]) == 0 && close(probe_pipe[1]) == 0,
          "spawn_pipe_close");

    FILE *stream = fopen(output, "r");
    char line[128];
    CHECK(stream != NULL, "spawn_output_open");
    CHECK(fgets(line, sizeof(line), stream) != NULL, "spawn_output_read");
    CHECK(strcmp(line, "LIBC_COMBINATION SPAWN_CHILD_OK\n") == 0,
          "spawn_output_marker");
    CHECK(fclose(stream) == 0, "spawn_output_close");
    CHECK(unlink(output) == 0, "spawn_output_unlink");

    child = (pid_t)-1;
    char *const missing_argv[] = {"respos-definitely-missing", NULL};
    spawn_result = posix_spawn(&child, "/respos-definitely-missing", NULL, NULL,
                               missing_argv, environ);
    CHECK(spawn_result == ENOENT, "spawn_missing_enoent");

    const char *slash = strrchr(self_path, '/');
    CHECK(slash != NULL && slash[1] != '\0', "spawnp_basename");
    char directory[4096];
    size_t directory_length = (size_t)(slash - self_path);
    CHECK(directory_length > 0 && directory_length < sizeof(directory),
          "spawnp_directory");
    memcpy(directory, self_path, directory_length);
    directory[directory_length] = '\0';
    const char *old_path = getenv("PATH");
    char path_value[8192];
    CHECK(snprintf(path_value, sizeof(path_value), "%s:%s", directory,
                   old_path != NULL ? old_path : "") < (int)sizeof(path_value),
          "spawnp_path_value");
    CHECK(setenv("PATH", path_value, 1) == 0, "spawnp_setenv");
    char *const spawnp_argv[] = {(char *)(slash + 1), "--exit-42", NULL};
    spawn_result = posix_spawnp(&child, slash + 1, NULL, NULL, spawnp_argv, environ);
    CHECK(spawn_result == 0, "posix_spawnp");
    CHECK(wait_child(child, 42), "spawnp_child_status");
    puts("LIBC_COMBINATION posix_spawn PASS");
    return 0;
}

int main(int argc, char **argv) {
    alarm(30);
    if (argc == 3 && strcmp(argv[1], "--spawn-child") == 0) {
        return spawn_child(atoi(argv[2]));
    }
    if (argc == 2 && strcmp(argv[1], "--exit-42") == 0) {
        return 42;
    }

    char self_path[4096];
    CHECK(realpath(argv[0], self_path) != NULL, "realpath_self");
    CHECK(test_pthread_core() == 0, "pthread_core_group");
    CHECK(test_pthread_detach_and_reuse() == 0, "pthread_detach_group");
    CHECK(test_pthread_robust() == 0, "pthread_robust_group");
    CHECK(test_pthread_pshared() == 0, "pthread_pshared_group");
    CHECK(test_pthread_fork_exec(self_path) == 0, "pthread_fork_exec_group");
    CHECK(test_posix_spawn(self_path) == 0, "posix_spawn_group");
    puts("LIBC_COMBINATION ALL PASS");
    return 0;
}
