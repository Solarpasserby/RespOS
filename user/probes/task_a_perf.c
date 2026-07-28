#define _GNU_SOURCE

#include <errno.h>
#include <linux/futex.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

enum {
    ROUNDS = 7,
    GETPID_ITERS = 100000,
    YIELD_ITERS = 10000,
    THREAD_ITERS = 200,
    FUTEX_FAST_ITERS = 20000,
    FUTEX_PING_PONG_ITERS = 2000,
    SLEEP_ITERS = 100,
    SLEEP_NS = 1000000,
};

static volatile long result_sink;

static uint64_t now_ns(void)
{
    struct timespec value;

    /*
     * The pre-refactor kernel exposes CLOCK_MONOTONIC at millisecond
     * granularity, while CLOCK_REALTIME already uses the microsecond clock.
     * No clock adjustment is performed during this isolated probe, so
     * CLOCK_REALTIME gives both kernels the same measurement resolution.
     */
    if (clock_gettime(CLOCK_REALTIME, &value) != 0) {
        perror("clock_gettime");
        exit(2);
    }
    return (uint64_t)value.tv_sec * 1000000000ULL + value.tv_nsec;
}

static long futex_wait(atomic_int *word, int expected)
{
    return syscall(SYS_futex, word, FUTEX_WAIT, expected, NULL, NULL, 0);
}

static long futex_wake(atomic_int *word, int count)
{
    return syscall(SYS_futex, word, FUTEX_WAKE, count, NULL, NULL, 0);
}

static void *empty_thread(void *argument)
{
    (void)argument;
    return NULL;
}

struct ping_pong {
    atomic_int turn;
    int iterations;
};

static void wait_for_turn(struct ping_pong *state, int wanted)
{
    while (atomic_load_explicit(&state->turn, memory_order_acquire) != wanted) {
        long ret = futex_wait(&state->turn, 1 - wanted);
        if (ret < 0 && errno != EAGAIN && errno != EINTR) {
            perror("futex wait");
            exit(3);
        }
    }
}

static void pass_turn(struct ping_pong *state, int next)
{
    atomic_store_explicit(&state->turn, next, memory_order_release);
    if (futex_wake(&state->turn, 1) < 0) {
        perror("futex wake");
        exit(4);
    }
}

static void *ping_pong_worker(void *argument)
{
    struct ping_pong *state = argument;

    for (int i = 0; i < state->iterations; ++i) {
        wait_for_turn(state, 1);
        pass_turn(state, 0);
    }
    return NULL;
}

static uint64_t bench_getpid(void)
{
    uint64_t start = now_ns();
    long sum = 0;

    for (int i = 0; i < GETPID_ITERS; ++i)
        sum += syscall(SYS_getpid);
    result_sink = sum;
    return (now_ns() - start) / GETPID_ITERS;
}

static uint64_t bench_yield(void)
{
    uint64_t start = now_ns();

    for (int i = 0; i < YIELD_ITERS; ++i) {
        if (syscall(SYS_sched_yield) != 0) {
            perror("sched_yield");
            exit(5);
        }
    }
    return (now_ns() - start) / YIELD_ITERS;
}

static uint64_t bench_thread_create_join(void)
{
    uint64_t start = now_ns();

    for (int i = 0; i < THREAD_ITERS; ++i) {
        pthread_t thread;
        int ret = pthread_create(&thread, NULL, empty_thread, NULL);
        if (ret != 0) {
            errno = ret;
            perror("pthread_create");
            exit(6);
        }
        ret = pthread_join(thread, NULL);
        if (ret != 0) {
            errno = ret;
            perror("pthread_join");
            exit(7);
        }
    }
    return (now_ns() - start) / THREAD_ITERS;
}

static uint64_t bench_futex_uncontended(void)
{
    atomic_int word = 0;
    uint64_t start = now_ns();

    for (int i = 0; i < FUTEX_FAST_ITERS; ++i) {
        long ret = futex_wake(&word, 1);
        if (ret != 0) {
            fprintf(stderr, "uncontended futex wake returned %ld\n", ret);
            exit(8);
        }
    }
    return (now_ns() - start) / FUTEX_FAST_ITERS;
}

static uint64_t bench_futex_contended(void)
{
    struct ping_pong state = {
        .turn = ATOMIC_VAR_INIT(0),
        .iterations = FUTEX_PING_PONG_ITERS,
    };
    pthread_t worker;
    int ret = pthread_create(&worker, NULL, ping_pong_worker, &state);
    if (ret != 0) {
        errno = ret;
        perror("pthread_create ping-pong");
        exit(9);
    }

    uint64_t start = now_ns();
    for (int i = 0; i < state.iterations; ++i) {
        wait_for_turn(&state, 0);
        pass_turn(&state, 1);
    }
    ret = pthread_join(worker, NULL);
    if (ret != 0) {
        errno = ret;
        perror("pthread_join ping-pong");
        exit(10);
    }

    /* One iteration contains a main->worker and worker->main hand-off. */
    return (now_ns() - start) / (2ULL * state.iterations);
}

static uint64_t bench_sleep_wakeup(void)
{
    const struct timespec request = {
        .tv_sec = 0,
        .tv_nsec = SLEEP_NS,
    };
    uint64_t start = now_ns();

    for (int i = 0; i < SLEEP_ITERS; ++i) {
        struct timespec remaining = request;
        while (nanosleep(&remaining, &remaining) != 0) {
            if (errno != EINTR) {
                perror("nanosleep");
                exit(11);
            }
        }
    }

    uint64_t elapsed = now_ns() - start;
    uint64_t requested = (uint64_t)SLEEP_ITERS * SLEEP_NS;
    return elapsed > requested ? (elapsed - requested) / SLEEP_ITERS : 0;
}

int main(void)
{
    puts("TASK_A_PERF version=1 unit=ns_per_op rounds=7 smp=1");
    for (int round = 1; round <= ROUNDS; ++round) {
        uint64_t getpid_ns = bench_getpid();
        uint64_t yield_ns = bench_yield();
        uint64_t thread_ns = bench_thread_create_join();
        uint64_t futex_fast_ns = bench_futex_uncontended();
        uint64_t futex_contended_ns = bench_futex_contended();
        uint64_t sleep_wakeup_ns = bench_sleep_wakeup();

        printf("TASK_A_PERF round=%d getpid=%llu yield=%llu "
               "pthread_create_join=%llu futex_uncontended=%llu "
               "futex_contended=%llu sleep_wakeup=%llu\n",
               round,
               (unsigned long long)getpid_ns,
               (unsigned long long)yield_ns,
               (unsigned long long)thread_ns,
               (unsigned long long)futex_fast_ns,
               (unsigned long long)futex_contended_ns,
               (unsigned long long)sleep_wakeup_ns);
    }
    puts("TASK_A_PERF PASS");
    return result_sink == -1;
}
