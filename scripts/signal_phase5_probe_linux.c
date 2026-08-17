#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/futex.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/epoll.h>
#include <sys/select.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

typedef uint64_t kernel_sigset_t;

static kernel_sigset_t signal_bit(int signo)
{
    return UINT64_C(1) << (signo - 1);
}

static volatile sig_atomic_t sigchld_count;
static volatile sig_atomic_t sigusr1_count;

static void count_sigchld(int signo)
{
    assert(signo == SIGCHLD);
    sigchld_count++;
}

static void count_sigusr1(int signo)
{
    assert(signo == SIGUSR1);
    sigusr1_count++;
}

static void test_sigchld_autoreap(void)
{
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    sigemptyset(&action.sa_mask);

    action.sa_handler = SIG_IGN;
    assert(sigaction(SIGCHLD, &action, NULL) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0)
        _exit(0);
    errno = 0;
    assert(waitpid(child, NULL, 0) == -1 && errno == ECHILD);

    sigchld_count = 0;
    action.sa_handler = count_sigchld;
    action.sa_flags = SA_NOCLDWAIT;
    assert(sigaction(SIGCHLD, &action, NULL) == 0);
    child = fork();
    assert(child >= 0);
    if (child == 0)
        _exit(0);
    errno = 0;
    assert(waitpid(child, NULL, 0) == -1 && errno == ECHILD);
    assert(sigchld_count == 1);

    action.sa_handler = SIG_DFL;
    action.sa_flags = 0;
    assert(sigaction(SIGCHLD, &action, NULL) == 0);
    child = fork();
    assert(child >= 0);
    if (child == 0)
        _exit(0);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    puts("SIGNAL_PHASE5_LINUX sigchld_autoreap PASS");
}

static void test_sigchld_nocldstop(void)
{
    sigchld_count = 0;
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    sigemptyset(&action.sa_mask);
    action.sa_handler = count_sigchld;
    action.sa_flags = SA_NOCLDSTOP;
    assert(sigaction(SIGCHLD, &action, NULL) == 0);

    int ready_pipe[2];
    assert(pipe(ready_pipe) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(close(ready_pipe[0]) == 0);
        char ready = 1;
        assert(write(ready_pipe[1], &ready, 1) == 1);
        assert(close(ready_pipe[1]) == 0);
        for (;;)
            pause();
    }
    assert(close(ready_pipe[1]) == 0);
    char ready = 0;
    assert(read(ready_pipe[0], &ready, 1) == 1 && ready == 1);
    assert(close(ready_pipe[0]) == 0);
    assert(kill(child, SIGSTOP) == 0);
    int status = 0;
    assert(waitpid(child, &status, WUNTRACED) == child);
    assert(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP);
    assert(sigchld_count == 0);
    assert(kill(child, SIGCONT) == 0);
    assert(kill(child, SIGKILL) == 0);
    assert(waitpid(child, &status, 0) == child);

    action.sa_handler = SIG_DFL;
    action.sa_flags = 0;
    assert(sigaction(SIGCHLD, &action, NULL) == 0);
    puts("SIGNAL_PHASE5_LINUX sigchld_nocldstop PASS");
}

static void test_query_ignores_how(void)
{
    kernel_sigset_t oldset = UINT64_MAX;
    assert(syscall(SYS_rt_sigprocmask, -1, NULL, &oldset, sizeof(oldset)) == 0);
    puts("SIGNAL_PHASE5_LINUX sigprocmask_query PASS");
}

static void test_sigaction_size_and_input_order(void)
{
    struct sigaction old_action;
    struct sigaction snapshot;
    memset(&old_action, 0x5a, sizeof(old_action));
    snapshot = old_action;

    errno = 0;
    assert(syscall(SYS_rt_sigaction, SIGUSR2, NULL, NULL, 0) == -1);
    assert(errno == EINVAL);

    errno = 0;
    assert(syscall(SYS_rt_sigaction, SIGUSR2, (void *)(uintptr_t)-1,
                   &old_action, sizeof(kernel_sigset_t)) == -1);
    assert(errno == EFAULT);
    assert(memcmp(&old_action, &snapshot, sizeof(old_action)) == 0);
    puts("SIGNAL_PHASE5_LINUX sigaction_validation PASS");
}

static void test_sigqueueinfo_null_signal(void)
{
    siginfo_t info;
    memset(&info, 0, sizeof(info));
    info.si_code = SI_QUEUE;
    info.si_pid = getpid();
    info.si_uid = getuid();
    assert(syscall(SYS_rt_sigqueueinfo, getpid(), 0, &info) == 0);
    puts("SIGNAL_PHASE5_LINUX sigqueueinfo_zero PASS");
}

static void queue_signal_value(int signo, int value)
{
    siginfo_t info;
    memset(&info, 0, sizeof(info));
    info.si_signo = signo;
    info.si_code = SI_QUEUE;
    info.si_pid = getpid();
    info.si_uid = getuid();
    info.si_value.sival_int = value;
    assert(syscall(SYS_rt_sigqueueinfo, getpid(), signo, &info) == 0);
}

static siginfo_t wait_signal_value(int signo)
{
    kernel_sigset_t set = signal_bit(signo);
    struct timespec timeout = {0, 0};
    siginfo_t info;
    memset(&info, 0, sizeof(info));
    assert(syscall(SYS_rt_sigtimedwait, &set, &info, &timeout,
                   sizeof(set)) == signo);
    return info;
}

static void test_pending_queue_semantics(void)
{
    kernel_sigset_t block = signal_bit(SIGUSR2) | signal_bit(SIGRTMIN);
    assert(syscall(SYS_rt_sigprocmask, SIG_BLOCK, &block, NULL,
                   sizeof(block)) == 0);

    queue_signal_value(SIGUSR2, 11);
    queue_signal_value(SIGUSR2, 22);
    siginfo_t info = wait_signal_value(SIGUSR2);
    assert(info.si_code == SI_QUEUE && info.si_value.sival_int == 11);
    kernel_sigset_t standard_set = signal_bit(SIGUSR2);
    struct timespec timeout = {0, 0};
    errno = 0;
    assert(syscall(SYS_rt_sigtimedwait, &standard_set, &info, &timeout,
                   sizeof(standard_set)) == -1);
    assert(errno == EAGAIN);

    const int values[] = {101, 202, 303};
    for (size_t i = 0; i < sizeof(values) / sizeof(values[0]); i++)
        queue_signal_value(SIGRTMIN, values[i]);
    for (size_t i = 0; i < sizeof(values) / sizeof(values[0]); i++) {
        info = wait_signal_value(SIGRTMIN);
        assert(info.si_signo == SIGRTMIN && info.si_code == SI_QUEUE);
        assert(info.si_value.sival_int == values[i]);
    }
    kernel_sigset_t realtime_set = signal_bit(SIGRTMIN);
    errno = 0;
    assert(syscall(SYS_rt_sigtimedwait, &realtime_set, &info, &timeout,
                   sizeof(realtime_set)) == -1);
    assert(errno == EAGAIN);
    puts("SIGNAL_PHASE5_LINUX pending_queue_semantics PASS");
}

static void run_pipe_read_signal_case(int restart, int default_ignored)
{
    int signo = default_ignored ? SIGWINCH : SIGUSR1;
    if (!default_ignored) {
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        sigemptyset(&action.sa_mask);
        action.sa_handler = count_sigusr1;
        action.sa_flags = restart ? SA_RESTART : 0;
        sigusr1_count = 0;
        assert(sigaction(signo, &action, NULL) == 0);
    }

    int fds[2];
    assert(pipe(fds) == 0);
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        close(fds[0]);
        usleep(20000);
        assert(kill(parent, signo) == 0);
        usleep(20000);
        unsigned char byte = 0x5a;
        assert(write(fds[1], &byte, 1) == 1);
        _exit(0);
    }

    close(fds[1]);
    unsigned char byte = 0;
    errno = 0;
    ssize_t first = read(fds[0], &byte, 1);
    if (!restart && !default_ignored) {
        assert(first == -1 && errno == EINTR);
        assert(read(fds[0], &byte, 1) == 1);
    } else {
        assert(first == 1);
    }
    assert(byte == 0x5a);
    close(fds[0]);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    if (!default_ignored) {
        assert(sigusr1_count == 1);
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        action.sa_handler = SIG_DFL;
        sigemptyset(&action.sa_mask);
        assert(sigaction(signo, &action, NULL) == 0);
    }
}

static void test_pipe_read_restart(void)
{
    run_pipe_read_signal_case(0, 0);
    run_pipe_read_signal_case(1, 0);
    run_pipe_read_signal_case(0, 1);
    puts("SIGNAL_PHASE5_LINUX pipe_read_restart PASS");
}

static void fill_pipe(int fd)
{
    int flags = fcntl(fd, F_GETFL);
    assert(flags >= 0);
    assert(fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0);
    unsigned char chunk[4096];
    memset(chunk, 0xa5, sizeof(chunk));
    size_t total = 0;
    for (;;) {
        ssize_t written = write(fd, chunk, sizeof(chunk));
        if (written > 0) {
            total += (size_t)written;
            continue;
        }
        assert(written == -1 && errno == EAGAIN);
        break;
    }
    assert(total != 0);
    assert(fcntl(fd, F_SETFL, flags) == 0);
}

static void run_pipe_write_signal_case(int restart, int default_ignored)
{
    int signo = default_ignored ? SIGWINCH : SIGUSR1;
    if (!default_ignored) {
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        sigemptyset(&action.sa_mask);
        action.sa_handler = count_sigusr1;
        action.sa_flags = restart ? SA_RESTART : 0;
        sigusr1_count = 0;
        assert(sigaction(signo, &action, NULL) == 0);
    }

    int fds[2];
    int ack[2];
    assert(pipe(fds) == 0);
    assert(pipe(ack) == 0);
    fill_pipe(fds[1]);
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        close(fds[1]);
        close(ack[1]);
        usleep(20000);
        assert(kill(parent, signo) == 0);
        usleep(20000);
        unsigned char drained[4096];
        assert(read(fds[0], drained, sizeof(drained)) == (ssize_t)sizeof(drained));
        assert(read(ack[0], drained, 1) == 1);
        _exit(0);
    }

    close(fds[0]);
    close(ack[0]);
    unsigned char byte = 0x5a;
    errno = 0;
    ssize_t first = write(fds[1], &byte, 1);
    if (!restart && !default_ignored) {
        assert(first == -1 && errno == EINTR);
        assert(write(fds[1], &byte, 1) == 1);
    } else {
        assert(first == 1);
    }
    assert(write(ack[1], &byte, 1) == 1);
    close(ack[1]);
    close(fds[1]);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    if (!default_ignored) {
        assert(sigusr1_count == 1);
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        action.sa_handler = SIG_DFL;
        sigemptyset(&action.sa_mask);
        assert(sigaction(signo, &action, NULL) == 0);
    }
}

static void test_pipe_write_restart(void)
{
    run_pipe_write_signal_case(0, 0);
    run_pipe_write_signal_case(1, 0);
    run_pipe_write_signal_case(0, 1);
    puts("SIGNAL_PHASE5_LINUX pipe_write_restart PASS");
}

static void run_pipe_readv_signal_case(int restart, int default_ignored)
{
    int signo = default_ignored ? SIGWINCH : SIGUSR1;
    if (!default_ignored) {
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        sigemptyset(&action.sa_mask);
        action.sa_handler = count_sigusr1;
        action.sa_flags = restart ? SA_RESTART : 0;
        sigusr1_count = 0;
        assert(sigaction(signo, &action, NULL) == 0);
    }

    int fds[2];
    assert(pipe(fds) == 0);
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        close(fds[0]);
        usleep(20000);
        assert(kill(parent, signo) == 0);
        usleep(20000);
        unsigned char byte = 0x6b;
        assert(write(fds[1], &byte, 1) == 1);
        _exit(0);
    }

    close(fds[1]);
    unsigned char bytes[2] = {0, 0};
    struct iovec iov[2] = {
        {.iov_base = &bytes[0], .iov_len = 1},
        {.iov_base = &bytes[1], .iov_len = 1},
    };
    errno = 0;
    ssize_t first = readv(fds[0], iov, 2);
    ssize_t result = first;
    if (!restart && !default_ignored) {
        assert(first == -1 && errno == EINTR);
        result = readv(fds[0], iov, 2);
    }
    assert(result == 1 && bytes[0] == 0x6b && bytes[1] == 0);
    close(fds[0]);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    if (!default_ignored) {
        assert(sigusr1_count == 1);
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        action.sa_handler = SIG_DFL;
        sigemptyset(&action.sa_mask);
        assert(sigaction(signo, &action, NULL) == 0);
    }
}

static void run_pipe_writev_signal_case(int restart, int default_ignored)
{
    int signo = default_ignored ? SIGWINCH : SIGUSR1;
    if (!default_ignored) {
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        sigemptyset(&action.sa_mask);
        action.sa_handler = count_sigusr1;
        action.sa_flags = restart ? SA_RESTART : 0;
        sigusr1_count = 0;
        assert(sigaction(signo, &action, NULL) == 0);
    }

    int fds[2];
    int ack[2];
    assert(pipe(fds) == 0);
    assert(pipe(ack) == 0);
    fill_pipe(fds[1]);
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        close(fds[1]);
        close(ack[1]);
        usleep(20000);
        assert(kill(parent, signo) == 0);
        usleep(20000);
        unsigned char drained[4096];
        assert(read(fds[0], drained, sizeof(drained)) == (ssize_t)sizeof(drained));
        assert(read(ack[0], drained, 1) == 1);
        _exit(0);
    }

    close(fds[0]);
    close(ack[0]);
    unsigned char bytes[2] = {0x71, 0x72};
    struct iovec iov[2] = {
        {.iov_base = &bytes[0], .iov_len = 1},
        {.iov_base = &bytes[1], .iov_len = 1},
    };
    errno = 0;
    ssize_t first = writev(fds[1], iov, 2);
    ssize_t result = first;
    if (!restart && !default_ignored) {
        assert(first == -1 && errno == EINTR);
        result = writev(fds[1], iov, 2);
    }
    assert(result == 2);
    unsigned char done = 1;
    assert(write(ack[1], &done, 1) == 1);
    close(ack[1]);
    close(fds[1]);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    if (!default_ignored) {
        assert(sigusr1_count == 1);
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        action.sa_handler = SIG_DFL;
        sigemptyset(&action.sa_mask);
        assert(sigaction(signo, &action, NULL) == 0);
    }
}

static void test_pipe_vectored_restart(void)
{
    const int modes[][2] = {{0, 0}, {1, 0}, {0, 1}};
    for (size_t i = 0; i < sizeof(modes) / sizeof(modes[0]); i++) {
        run_pipe_readv_signal_case(modes[i][0], modes[i][1]);
        run_pipe_writev_signal_case(modes[i][0], modes[i][1]);
    }
    puts("SIGNAL_PHASE5_LINUX pipe_vectored_restart PASS");
}

static void run_futex_signal_case(int restart, int default_ignored)
{
    int signo = default_ignored ? SIGWINCH : SIGUSR1;
    if (!default_ignored) {
        sigusr1_count = 0;
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        action.sa_handler = count_sigusr1;
        action.sa_flags = restart ? SA_RESTART : 0;
        sigemptyset(&action.sa_mask);
        assert(sigaction(signo, &action, NULL) == 0);
    }

    int *futex = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                      MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    assert(futex != MAP_FAILED);
    *futex = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(20000);
        assert(kill(parent, signo) == 0);
        if (restart || default_ignored) {
            usleep(20000);
            assert(syscall(SYS_futex, futex, FUTEX_WAKE, 1, NULL, NULL, 0) == 1);
        }
        _exit(0);
    }

    errno = 0;
    long result = syscall(SYS_futex, futex, FUTEX_WAIT, 0, NULL, NULL, 0);
    if (restart || default_ignored)
        assert(result == 0);
    else
        assert(result == -1 && errno == EINTR);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(default_ignored || sigusr1_count == 1);
    assert(munmap(futex, 4096) == 0);
    if (!default_ignored) {
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        action.sa_handler = SIG_DFL;
        sigemptyset(&action.sa_mask);
        assert(sigaction(signo, &action, NULL) == 0);
    }
}

static void test_futex_restart(void)
{
    run_futex_signal_case(0, 0);
    run_futex_signal_case(1, 0);
    run_futex_signal_case(0, 1);
    puts("SIGNAL_PHASE5_LINUX futex_restart PASS");
}

static long monotonic_ms(void)
{
    struct timespec now;
    assert(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
    return now.tv_sec * 1000L + now.tv_nsec / 1000000L;
}

static void install_timeout_signal_action(int signo, int restart)
{
    if (signo != SIGUSR1)
        return;
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = count_sigusr1;
    action.sa_flags = restart ? SA_RESTART : 0;
    sigemptyset(&action.sa_mask);
    assert(sigaction(signo, &action, NULL) == 0);
    sigusr1_count = 0;
}

static void run_timeout_signal_case(int wait_kind, int restart,
                                    int default_ignored)
{
    int signo = default_ignored ? SIGWINCH : SIGUSR1;
    install_timeout_signal_action(signo, restart);
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, signo) == 0);
        _exit(0);
    }

    struct timespec request = {.tv_sec = 0, .tv_nsec = 400000000};
    struct timespec remaining = {.tv_sec = 9, .tv_nsec = 9};
    long started = monotonic_ms();
    errno = 0;
    int epfd = -1;
    int result;
    if (wait_kind == 0) {
        result = nanosleep(&request, &remaining);
    } else if (wait_kind == 1) {
        result = ppoll(NULL, 0, &request, NULL);
    } else if (wait_kind == 2) {
        result = pselect(0, NULL, NULL, NULL, &request, NULL);
    } else if (wait_kind == 3) {
        struct epoll_event event;
        epfd = epoll_create1(0);
        assert(epfd >= 0);
        result = epoll_pwait(epfd, &event, 1, 400, NULL);
    } else if (wait_kind == 4) {
        result = syscall(SYS_clock_nanosleep, CLOCK_MONOTONIC, 0,
                         &request, &remaining);
    } else {
        struct timespec deadline;
        assert(clock_gettime(CLOCK_MONOTONIC, &deadline) == 0);
        deadline.tv_nsec += 400000000;
        if (deadline.tv_nsec >= 1000000000) {
            deadline.tv_sec++;
            deadline.tv_nsec -= 1000000000;
        }
        result = syscall(SYS_clock_nanosleep, CLOCK_MONOTONIC,
                         TIMER_ABSTIME, &deadline, &remaining);
    }
    int saved_errno = errno;
    long elapsed = monotonic_ms() - started;
    if (default_ignored) {
        assert(result == 0);
        assert(elapsed >= 300);
    } else {
        assert(result == -1 && saved_errno == EINTR);
        assert(elapsed < 300);
        assert(sigusr1_count == 1);
        if (wait_kind == 0 || wait_kind == 4) {
            long remaining_ms = remaining.tv_sec * 1000L +
                                remaining.tv_nsec / 1000000L;
            assert(remaining_ms > 100 && remaining_ms <= 400);
            assert(elapsed + remaining_ms >= 300);
            assert(elapsed + remaining_ms <= 500);
        } else if (wait_kind == 5) {
            assert(remaining.tv_sec == 9 && remaining.tv_nsec == 9);
        }
    }
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    if (epfd >= 0)
        assert(close(epfd) == 0);
}

static void test_timeout_nonrestart(void)
{
    for (int wait_kind = 0; wait_kind < 6; ++wait_kind) {
        run_timeout_signal_case(wait_kind, 0, 0);
        run_timeout_signal_case(wait_kind, 1, 0);
        run_timeout_signal_case(wait_kind, 0, 1);
    }
    puts("SIGNAL_PHASE5_LINUX timeout_nonrestart PASS");
}

static void exec_pending_target(void)
{
    kernel_sigset_t mask = 0;
    kernel_sigset_t pending = 0;
    assert(syscall(SYS_rt_sigprocmask, -1, NULL, &mask, sizeof(mask)) == 0);
    assert(syscall(SYS_rt_sigpending, &pending, sizeof(pending)) == 0);
    assert((mask & signal_bit(SIGUSR1)) != 0);
    assert((pending & signal_bit(SIGUSR1)) != 0);
    puts("SIGNAL_PHASE5_LINUX exec_pending PASS");
    puts("SIGNAL_PHASE5_LINUX ALL PASS");
}

static void test_pending_survives_exec(char *self)
{
    kernel_sigset_t block = signal_bit(SIGUSR1);
    kernel_sigset_t pending = 0;
    assert(syscall(SYS_rt_sigprocmask, SIG_BLOCK, &block, NULL, sizeof(block)) == 0);
    assert(kill(getpid(), SIGUSR1) == 0);
    assert(syscall(SYS_rt_sigpending, &pending, sizeof(pending)) == 0);
    assert((pending & block) != 0);

    char *const argv[] = {self, (char *)"--exec-target", NULL};
    execv(self, argv);
    assert(!"execv failed");
}

int main(int argc, char **argv)
{
    setbuf(stdout, NULL);
    if (argc == 2 && strcmp(argv[1], "--exec-target") == 0) {
        exec_pending_target();
        return 0;
    }

    assert(argc >= 1);
    test_query_ignores_how();
    test_sigaction_size_and_input_order();
    test_sigqueueinfo_null_signal();
    test_pending_queue_semantics();
    test_pipe_read_restart();
    test_pipe_write_restart();
    test_pipe_vectored_restart();
    test_futex_restart();
    test_timeout_nonrestart();
    test_sigchld_autoreap();
    test_sigchld_nocldstop();
    test_pending_survives_exec(argv[0]);
    return 1;
}
