#define _GNU_SOURCE

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile sig_atomic_t sigpipe_count;
static volatile sig_atomic_t sigusr1_count;

static void fail(const char *what)
{
    perror(what);
    exit(1);
}

static void expect(int condition, const char *what)
{
    if (!condition) {
        fprintf(stderr, "FAIL: %s\n", what);
        exit(1);
    }
}

static void on_sigpipe(int signo)
{
    (void)signo;
    ++sigpipe_count;
}

static void on_sigusr1(int signo)
{
    (void)signo;
    ++sigusr1_count;
}

static void test_peek(void)
{
    int sv[2];
    char buf[8] = {0};

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair(MSG_PEEK)");
    if (send(sv[0], "abc", 3, 0) != 3)
        fail("send(MSG_PEEK)");
    expect(recv(sv[1], buf, 2, MSG_PEEK) == 2, "MSG_PEEK length");
    expect(memcmp(buf, "ab", 2) == 0, "MSG_PEEK bytes");
    memset(buf, 0, sizeof(buf));
    expect(recv(sv[1], buf, 3, 0) == 3, "read after MSG_PEEK length");
    expect(memcmp(buf, "abc", 3) == 0, "MSG_PEEK does not consume data");
    close(sv[0]);
    close(sv[1]);
}

static void test_waitall_fragmented(void)
{
    int sv[2];
    char buf[8] = {0};
    pid_t pid;

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair(MSG_WAITALL fragmented)");
    pid = fork();
    if (pid < 0)
        fail("fork(MSG_WAITALL fragmented)");
    if (pid == 0) {
        close(sv[0]);
        if (send(sv[1], "ab", 2, 0) != 2)
            _exit(2);
        usleep(50000);
        if (send(sv[1], "cd", 2, 0) != 2)
            _exit(3);
        close(sv[1]);
        _exit(0);
    }

    close(sv[1]);
    expect(recv(sv[0], buf, 4, MSG_WAITALL) == 4,
           "MSG_WAITALL waits across fragmented writes");
    expect(memcmp(buf, "abcd", 4) == 0, "MSG_WAITALL fragmented bytes");
    close(sv[0]);
    {
        int status;
        if (waitpid(pid, &status, 0) < 0)
            fail("waitpid(MSG_WAITALL fragmented)");
        expect(WIFEXITED(status) && WEXITSTATUS(status) == 0,
               "MSG_WAITALL writer exits cleanly");
    }
}

static void test_waitall_partial_timeout(void)
{
    int sv[2];
    char buf[8] = {0};
    struct timeval timeout = {.tv_sec = 0, .tv_usec = 100000};

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair(MSG_WAITALL timeout)");
    if (setsockopt(sv[1], SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) < 0)
        fail("setsockopt(SO_RCVTIMEO)");
    if (send(sv[0], "xy", 2, 0) != 2)
        fail("send(MSG_WAITALL timeout)");
    expect(recv(sv[1], buf, 4, MSG_WAITALL) == 2,
           "MSG_WAITALL timeout returns partial bytes");
    expect(memcmp(buf, "xy", 2) == 0, "MSG_WAITALL timeout partial bytes");
    close(sv[0]);
    close(sv[1]);
}

static void test_waitall_partial_eof(void)
{
    int sv[2];
    char buf[8] = {0};

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair(MSG_WAITALL EOF)");
    if (send(sv[0], "pq", 2, 0) != 2)
        fail("send(MSG_WAITALL EOF)");
    if (shutdown(sv[0], SHUT_WR) < 0)
        fail("shutdown(MSG_WAITALL EOF)");
    expect(recv(sv[1], buf, 4, MSG_WAITALL) == 2,
           "MSG_WAITALL EOF returns partial bytes");
    expect(memcmp(buf, "pq", 2) == 0, "MSG_WAITALL EOF partial bytes");
    close(sv[0]);
    close(sv[1]);
}

static void test_waitall_partial_signal(void)
{
    struct sigaction action;
    int sv[2];
    char buf[8] = {0};
    pid_t parent = getpid();
    pid_t pid;

    memset(&action, 0, sizeof(action));
    action.sa_handler = on_sigusr1;
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR1, &action, NULL) < 0)
        fail("sigaction(SIGUSR1)");
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair(MSG_WAITALL signal)");
    pid = fork();
    if (pid < 0)
        fail("fork(MSG_WAITALL signal)");
    if (pid == 0) {
        close(sv[0]);
        if (send(sv[1], "uv", 2, 0) != 2)
            _exit(2);
        usleep(50000);
        if (kill(parent, SIGUSR1) < 0)
            _exit(3);
        usleep(50000);
        close(sv[1]);
        _exit(0);
    }

    close(sv[1]);
    expect(recv(sv[0], buf, 4, MSG_WAITALL) == 2,
           "MSG_WAITALL signal returns partial bytes");
    expect(memcmp(buf, "uv", 2) == 0, "MSG_WAITALL signal partial bytes");
    expect(sigusr1_count == 1, "MSG_WAITALL signal handler ran");
    close(sv[0]);
    {
        int status;
        if (waitpid(pid, &status, 0) < 0)
            fail("waitpid(MSG_WAITALL signal)");
        expect(WIFEXITED(status) && WEXITSTATUS(status) == 0,
               "MSG_WAITALL signaler exits cleanly");
    }
}

static void test_nosignal(void)
{
    struct sigaction action;
    int sv[2];

    memset(&action, 0, sizeof(action));
    action.sa_handler = on_sigpipe;
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGPIPE, &action, NULL) < 0)
        fail("sigaction(SIGPIPE)");

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair(SIGPIPE)");
    close(sv[1]);
    errno = 0;
    expect(send(sv[0], "x", 1, 0) == -1 && errno == EPIPE,
           "send on closed peer returns EPIPE");
    expect(sigpipe_count == 1, "send without MSG_NOSIGNAL raises SIGPIPE");
    close(sv[0]);

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair(MSG_NOSIGNAL)");
    close(sv[1]);
    errno = 0;
    expect(send(sv[0], "x", 1, MSG_NOSIGNAL) == -1 && errno == EPIPE,
           "MSG_NOSIGNAL send on closed peer returns EPIPE");
    expect(sigpipe_count == 1, "MSG_NOSIGNAL suppresses SIGPIPE");
    close(sv[0]);
}

int main(void)
{
    test_peek();
    test_waitall_fragmented();
    test_waitall_partial_timeout();
    test_waitall_partial_eof();
    test_waitall_partial_signal();
    test_nosignal();
    puts("socket flags Linux probe: PASS");
    return 0;
}
