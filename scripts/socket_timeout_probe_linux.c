#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static long monotonic_ms(void)
{
    struct timespec now;
    assert(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
    return now.tv_sec * 1000L + now.tv_nsec / 1000000L;
}

static void wait_child(pid_t child)
{
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

static void test_recv_timeout_and_timeval_abi(void)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);

    struct timeval timeout = {.tv_sec = 0, .tv_usec = 50000};
    if (setsockopt(sv[0], SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) {
        perror("setsockopt(SO_RCVTIMEO)");
        abort();
    }

    struct timeval observed = {0};
    socklen_t observed_len = sizeof(observed);
    assert(getsockopt(sv[0], SOL_SOCKET, SO_RCVTIMEO, &observed, &observed_len) == 0);
    assert(observed_len == sizeof(observed));
    assert(observed.tv_sec == 0);
    assert(observed.tv_usec >= 40000 && observed.tv_usec <= 60000);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(close(sv[0]) == 0);
        usleep(200000);
        assert(write(sv[1], "x", 1) == 1);
        _exit(0);
    }
    assert(close(sv[1]) == 0);

    char byte = 0;
    errno = 0;
    long start = monotonic_ms();
    assert(recv(sv[0], &byte, 1, 0) == -1);
    long elapsed = monotonic_ms() - start;
    assert(errno == EAGAIN || errno == EWOULDBLOCK);
    assert(elapsed >= 35 && elapsed < 180);

    wait_child(child);
    assert(close(sv[0]) == 0);
    puts("SOCKET_TIMEOUT_LINUX recv_timeout_timeval PASS");
}

static void test_zero_timeout_and_msg_dontwait(void)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);

    struct timeval zero = {0};
    assert(setsockopt(sv[0], SOL_SOCKET, SO_RCVTIMEO, &zero, sizeof(zero)) == 0);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(close(sv[0]) == 0);
        usleep(50000);
        assert(write(sv[1], "z", 1) == 1);
        _exit(0);
    }
    assert(close(sv[1]) == 0);

    char byte = 0;
    errno = 0;
    long start = monotonic_ms();
    assert(recv(sv[0], &byte, 1, MSG_DONTWAIT) == -1);
    long elapsed = monotonic_ms() - start;
    assert(errno == EAGAIN || errno == EWOULDBLOCK);
    assert(elapsed < 30);

    assert(recv(sv[0], &byte, 1, 0) == 1);
    assert(byte == 'z');
    wait_child(child);
    assert(close(sv[0]) == 0);
    puts("SOCKET_TIMEOUT_LINUX zero_and_dontwait PASS");
}

static void test_send_timeout(void)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);

    int small_buffer = 4096;
    assert(setsockopt(sv[0], SOL_SOCKET, SO_SNDBUF, &small_buffer, sizeof(small_buffer)) == 0);
    struct timeval timeout = {.tv_sec = 0, .tv_usec = 50000};
    assert(setsockopt(sv[0], SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) == 0);

    char payload[4096];
    memset(payload, 0x5a, sizeof(payload));
    size_t total = 0;
    for (;;) {
        errno = 0;
        long start = monotonic_ms();
        ssize_t written = send(sv[0], payload, sizeof(payload), MSG_NOSIGNAL);
        long elapsed = monotonic_ms() - start;
        if (written >= 0) {
            total += (size_t)written;
            continue;
        }
        assert(errno == EAGAIN || errno == EWOULDBLOCK);
        assert(elapsed >= 35 && elapsed < 180);
        break;
    }
    assert(total > 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
    puts("SOCKET_TIMEOUT_LINUX send_timeout PASS");
}

int main(void)
{
    setbuf(stdout, NULL);
    assert(signal(SIGPIPE, SIG_IGN) != SIG_ERR);
    test_recv_timeout_and_timeval_abi();
    test_zero_timeout_and_msg_dontwait();
    test_send_timeout();
    puts("SOCKET_TIMEOUT_LINUX ALL PASS");
    return 0;
}
