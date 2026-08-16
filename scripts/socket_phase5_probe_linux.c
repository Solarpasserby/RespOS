#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/epoll.h>
#include <sys/un.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t signal_seen;

static long monotonic_ms(void)
{
    struct timespec now;
    assert(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
    return now.tv_sec * 1000L + now.tv_nsec / 1000000L;
}

static void signal_handler(int signo)
{
    (void)signo;
    signal_seen = 1;
}

static socklen_t make_addr(struct sockaddr_un *addr, const char *path)
{
    memset(addr, 0, sizeof(*addr));
    addr->sun_family = AF_UNIX;
    assert(strlen(path) < sizeof(addr->sun_path));
    strcpy(addr->sun_path, path);
    return (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(path) + 1);
}

static void test_pathname_and_nonblock(const char *path)
{
    struct sockaddr_un addr;
    socklen_t addrlen = make_addr(&addr, path);
    int listener = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0);
    assert(listener >= 0);
    assert(bind(listener, (struct sockaddr *)&addr, addrlen) == 0);
    assert(listen(listener, 4) == 0);

    errno = 0;
    assert(accept(listener, NULL, NULL) == -1);
    assert(errno == EAGAIN || errno == EWOULDBLOCK);

    int client = socket(AF_UNIX, SOCK_STREAM, 0);
    assert(client >= 0);
    assert(connect(client, (struct sockaddr *)&addr, addrlen) == 0);
    int server = accept(listener, NULL, NULL);
    assert(server >= 0);

    const char payload[] = "pathname";
    char received[sizeof(payload)] = {0};
    assert(write(client, payload, sizeof(payload)) == (ssize_t)sizeof(payload));
    assert(read(server, received, sizeof(received)) == (ssize_t)sizeof(received));
    assert(memcmp(received, payload, sizeof(payload)) == 0);

    assert(close(client) == 0);
    assert(read(server, received, sizeof(received)) == 0);
    errno = 0;
    assert(write(server, payload, sizeof(payload)) == -1);
    assert(errno == EPIPE);
    assert(close(server) == 0);
    assert(close(listener) == 0);

    client = socket(AF_UNIX, SOCK_STREAM, 0);
    assert(client >= 0);
    errno = 0;
    assert(connect(client, (struct sockaddr *)&addr, addrlen) == -1);
    assert(errno == ECONNREFUSED);
    assert(close(client) == 0);
    puts("SOCKET_PHASE5_LINUX pathname_nonblock_eof PASS");
}

static void test_shutdown_and_poll(void)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    assert(write(sv[0], "b", 1) == 1);
    assert(shutdown(sv[0], SHUT_WR) == 0);

    struct pollfd pfd = {.fd = sv[1], .events = POLLIN | POLLOUT | POLLRDHUP};
    assert(poll(&pfd, 1, 0) == 1);
    assert((pfd.revents & POLLIN) != 0);
    assert((pfd.revents & POLLOUT) != 0);
    assert((pfd.revents & POLLRDHUP) != 0);
    assert((pfd.revents & POLLHUP) == 0);

    int epfd = epoll_create1(0);
    assert(epfd >= 0);
    struct epoll_event interest = {
        .events = EPOLLIN | EPOLLRDHUP,
        .data.u64 = UINT64_C(0x5244485550),
    };
    assert(epoll_ctl(epfd, EPOLL_CTL_ADD, sv[1], &interest) == 0);
    struct epoll_event ready = {0};
    assert(epoll_wait(epfd, &ready, 1, 0) == 1);
    assert((ready.events & EPOLLIN) != 0);
    assert((ready.events & EPOLLRDHUP) != 0);
    assert(ready.data.u64 == UINT64_C(0x5244485550));
    assert(close(epfd) == 0);

    char byte = 0;
    assert(read(sv[1], &byte, 1) == 1);
    assert(byte == 'b');
    assert(read(sv[1], &byte, 1) == 0);
    errno = 0;
    assert(write(sv[0], "x", 1) == -1);
    assert(errno == EPIPE);
    assert(write(sv[1], "y", 1) == 1);
    assert(read(sv[0], &byte, 1) == 1);
    assert(byte == 'y');

    assert(close(sv[0]) == 0);
    pfd.revents = 0;
    assert(poll(&pfd, 1, 0) == 1);
    assert((pfd.revents & POLLIN) != 0);
    assert((pfd.revents & POLLHUP) != 0);
    assert(close(sv[1]) == 0);
    puts("SOCKET_PHASE5_LINUX shutdown_poll_rdhup PASS");
}

static void test_rdhup_blocking_and_epoll_modes(void)
{
    int sv[2];
    int ack[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    assert(pipe(ack) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(close(sv[1]) == 0);
        assert(close(ack[1]) == 0);
        usleep(50000);
        assert(write(sv[0], "p", 1) == 1);
        assert(shutdown(sv[0], SHUT_WR) == 0);
        char byte;
        assert(read(ack[0], &byte, 1) == 1);
        _exit(0);
    }
    assert(close(sv[0]) == 0);
    assert(close(ack[0]) == 0);
    struct pollfd pfd = {.fd = sv[1], .events = POLLRDHUP};
    assert(poll(&pfd, 1, 1000) == 1);
    assert((pfd.revents & POLLRDHUP) != 0);
    assert((pfd.revents & (POLLIN | POLLHUP)) == 0);
    char byte = 0;
    assert(read(sv[1], &byte, 1) == 1 && byte == 'p');
    assert(write(ack[1], "a", 1) == 1);
    assert(close(ack[1]) == 0);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[1]) == 0);

    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    assert(pipe(ack) == 0);
    int epfd = epoll_create1(0);
    assert(epfd >= 0);
    struct epoll_event interest = {
        .events = EPOLLRDHUP,
        .data.u64 = UINT64_C(0x1001),
    };
    assert(epoll_ctl(epfd, EPOLL_CTL_ADD, sv[1], &interest) == 0);
    child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(close(sv[1]) == 0);
        assert(close(epfd) == 0);
        assert(close(ack[1]) == 0);
        usleep(50000);
        assert(write(sv[0], "e", 1) == 1);
        assert(shutdown(sv[0], SHUT_WR) == 0);
        char child_byte;
        assert(read(ack[0], &child_byte, 1) == 1);
        _exit(0);
    }
    assert(close(sv[0]) == 0);
    assert(close(ack[0]) == 0);
    struct epoll_event ready = {0};
    assert(epoll_wait(epfd, &ready, 1, 1000) == 1);
    assert(ready.events == EPOLLRDHUP);
    assert(ready.data.u64 == UINT64_C(0x1001));

    interest.events = EPOLLRDHUP | EPOLLET;
    interest.data.u64 = UINT64_C(0x2002);
    assert(epoll_ctl(epfd, EPOLL_CTL_MOD, sv[1], &interest) == 0);
    assert(epoll_wait(epfd, &ready, 1, 0) == 1);
    assert(ready.events == EPOLLRDHUP);
    assert(ready.data.u64 == UINT64_C(0x2002));
    assert(epoll_wait(epfd, &ready, 1, 0) == 0);

    interest.events = EPOLLRDHUP | EPOLLONESHOT;
    interest.data.u64 = UINT64_C(0x3003);
    assert(epoll_ctl(epfd, EPOLL_CTL_MOD, sv[1], &interest) == 0);
    assert(epoll_wait(epfd, &ready, 1, 0) == 1);
    assert(ready.events == EPOLLRDHUP);
    assert(ready.data.u64 == UINT64_C(0x3003));
    assert(epoll_wait(epfd, &ready, 1, 0) == 0);
    assert(epoll_ctl(epfd, EPOLL_CTL_MOD, sv[1], &interest) == 0);
    assert(epoll_wait(epfd, &ready, 1, 0) == 1);
    assert(ready.events == EPOLLRDHUP);

    assert(read(sv[1], &byte, 1) == 1 && byte == 'e');
    assert(write(ack[1], "a", 1) == 1);
    assert(close(ack[1]) == 0);
    status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[1]) == 0);
    assert(close(epfd) == 0);
    puts("SOCKET_PHASE5_LINUX rdhup_blocking_edge_oneshot PASS");
}

static void test_blocking_poll_and_pipe_events(void)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(close(sv[0]) == 0);
        usleep(50000);
        assert(write(sv[1], "w", 1) == 1);
        _exit(0);
    }
    assert(close(sv[1]) == 0);
    struct pollfd pfd = {.fd = sv[0], .events = POLLIN};
    assert(poll(&pfd, 1, 1000) == 1);
    assert((pfd.revents & POLLIN) != 0);
    char byte;
    assert(read(sv[0], &byte, 1) == 1 && byte == 'w');
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);

    int pipefds[2];
    assert(pipe(pipefds) == 0);
    pfd.fd = pipefds[0];
    pfd.events = 0;
    pfd.revents = 0;
    assert(poll(&pfd, 1, 0) == 0);
    assert(close(pipefds[1]) == 0);
    assert(poll(&pfd, 1, 0) == 1);
    assert((pfd.revents & POLLHUP) != 0);
    assert(close(pipefds[0]) == 0);

    assert(pipe(pipefds) == 0);
    pfd.fd = pipefds[1];
    pfd.events = 0;
    pfd.revents = 0;
    assert(close(pipefds[0]) == 0);
    assert(poll(&pfd, 1, 0) == 1);
    assert((pfd.revents & POLLERR) != 0);
    assert(close(pipefds[1]) == 0);

    assert(pipe(pipefds) == 0);
    int epfd = epoll_create1(0);
    assert(epfd >= 0);
    struct epoll_event interest = {.events = 0, .data.u64 = 0x12345678};
    assert(epoll_ctl(epfd, EPOLL_CTL_ADD, pipefds[0], &interest) == 0);
    assert(close(pipefds[1]) == 0);
    struct epoll_event ready;
    assert(epoll_wait(epfd, &ready, 1, 0) == 1);
    assert((ready.events & EPOLLHUP) != 0);
    assert(ready.data.u64 == UINT64_C(0x12345678));
    assert(close(pipefds[0]) == 0);
    assert(close(epfd) == 0);
    puts("SOCKET_PHASE5_LINUX blocking_poll_pipe_events PASS");
}

static void test_accept_signal(const char *path, int signo, int restart,
                               int recv_timeout, int expect_eintr,
                               int expect_handler)
{
    struct sockaddr_un addr;
    socklen_t addrlen = make_addr(&addr, path);
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    int ready[2];
    assert(listener >= 0);
    assert(bind(listener, (struct sockaddr *)&addr, addrlen) == 0);
    assert(listen(listener, 4) == 0);
    if (recv_timeout) {
        struct timeval timeout = {.tv_sec = 2, .tv_usec = 0};
        assert(setsockopt(listener, SOL_SOCKET, SO_RCVTIMEO, &timeout,
                          sizeof(timeout)) == 0);
    }
    assert(pipe(ready) == 0);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        if (signo == SIGUSR1) {
            struct sigaction action;
            memset(&action, 0, sizeof(action));
            action.sa_handler = signal_handler;
            action.sa_flags = restart ? SA_RESTART : 0;
            sigemptyset(&action.sa_mask);
            assert(sigaction(SIGUSR1, &action, NULL) == 0);
        }
        assert(close(ready[0]) == 0);
        assert(write(ready[1], "r", 1) == 1);
        assert(close(ready[1]) == 0);

        errno = 0;
        int accepted = accept(listener, NULL, NULL);
        if (expect_eintr) {
            assert(accepted == -1);
            assert(errno == EINTR);
        } else {
            assert(accepted >= 0);
            assert(close(accepted) == 0);
        }
        assert(signal_seen == expect_handler);
        _exit(0);
    }

    assert(close(ready[1]) == 0);
    char byte;
    assert(read(ready[0], &byte, 1) == 1);
    assert(close(ready[0]) == 0);
    usleep(50000);
    assert(kill(child, signo) == 0);
    if (!expect_eintr) {
        usleep(50000);
        int client = socket(AF_UNIX, SOCK_STREAM, 0);
        assert(client >= 0);
        assert(connect(client, (struct sockaddr *)&addr, addrlen) == 0);
        assert(close(client) == 0);
    }
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(listener) == 0);
}

static size_t fill_unix_accept_queue(const struct sockaddr_un *addr,
                                     socklen_t addrlen, int *fillers,
                                     size_t capacity)
{
    for (size_t count = 0; count < capacity; ++count) {
        int client = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0);
        assert(client >= 0);
        errno = 0;
        if (connect(client, (const struct sockaddr *)addr, addrlen) == 0) {
            fillers[count] = client;
            continue;
        }
        assert(errno == EAGAIN || errno == EWOULDBLOCK);
        assert(close(client) == 0);
        return count;
    }
    assert(!"AF_UNIX accept queue did not fill");
    return 0;
}

static void test_connect_signal(const char *path, int signo, int restart,
                                int send_timeout, int expect_eintr,
                                int expect_handler)
{
    struct sockaddr_un addr;
    socklen_t addrlen = make_addr(&addr, path);
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    int ready[2];
    int fillers[256];
    assert(listener >= 0);
    assert(bind(listener, (struct sockaddr *)&addr, addrlen) == 0);
    assert(listen(listener, 4) == 0);
    size_t filler_count =
        fill_unix_accept_queue(&addr, addrlen, fillers,
                               sizeof(fillers) / sizeof(fillers[0]));
    assert(filler_count > 0);
    assert(pipe(ready) == 0);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        if (signo == SIGUSR1) {
            struct sigaction action;
            memset(&action, 0, sizeof(action));
            action.sa_handler = signal_handler;
            action.sa_flags = restart ? SA_RESTART : 0;
            sigemptyset(&action.sa_mask);
            assert(sigaction(SIGUSR1, &action, NULL) == 0);
        }
        assert(close(ready[0]) == 0);
        int client = socket(AF_UNIX, SOCK_STREAM, 0);
        assert(client >= 0);
        if (send_timeout) {
            struct timeval timeout = {.tv_sec = 2, .tv_usec = 0};
            assert(setsockopt(client, SOL_SOCKET, SO_SNDTIMEO, &timeout,
                              sizeof(timeout)) == 0);
        }
        assert(write(ready[1], "r", 1) == 1);
        assert(close(ready[1]) == 0);
        errno = 0;
        int connected = connect(client, (struct sockaddr *)&addr, addrlen);
        if (expect_eintr) {
            assert(connected == -1);
            assert(errno == EINTR);
        } else {
            assert(connected == 0);
        }
        assert(signal_seen == expect_handler);
        assert(close(client) == 0);
        _exit(0);
    }

    assert(close(ready[1]) == 0);
    char byte;
    assert(read(ready[0], &byte, 1) == 1);
    assert(close(ready[0]) == 0);
    usleep(50000);
    assert(kill(child, signo) == 0);
    if (!expect_eintr) {
        usleep(50000);
        int accepted = accept(listener, NULL, NULL);
        assert(accepted >= 0);
        assert(close(accepted) == 0);
    }
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    for (size_t index = 0; index < filler_count; ++index)
        assert(close(fillers[index]) == 0);
    assert(close(listener) == 0);
}

static void install_parent_signal_action(int signo, int restart)
{
    if (signo != SIGUSR1)
        return;
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = signal_handler;
    action.sa_flags = restart ? SA_RESTART : 0;
    sigemptyset(&action.sa_mask);
    assert(sigaction(signo, &action, NULL) == 0);
}

static void test_recvfrom_signal(int signo, int restart, int recv_timeout,
                                 int expect_eintr, int expect_handler)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    if (recv_timeout) {
        struct timeval timeout = {.tv_sec = 2, .tv_usec = 0};
        assert(setsockopt(sv[0], SOL_SOCKET, SO_RCVTIMEO, &timeout,
                          sizeof(timeout)) == 0);
    }
    install_parent_signal_action(signo, restart);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, signo) == 0);
        if (!expect_eintr) {
            usleep(50000);
            assert(sendto(sv[1], "r", 1, 0, NULL, 0) == 1);
        }
        _exit(0);
    }

    char byte = 0;
    errno = 0;
    ssize_t received = recvfrom(sv[0], &byte, 1, 0, NULL, NULL);
    if (expect_eintr)
        assert(received == -1 && errno == EINTR);
    else
        assert(received == 1 && byte == 'r');
    assert(signal_seen == expect_handler);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

static void test_sendto_signal(int signo, int restart, int send_timeout,
                               int expect_eintr, int expect_handler)
{
    int sv[2];
    char chunk[4096] = {0};
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    for (;;) {
        ssize_t sent = sendto(sv[0], chunk, sizeof(chunk), MSG_DONTWAIT, NULL, 0);
        if (sent < 0) {
            assert(errno == EAGAIN || errno == EWOULDBLOCK);
            break;
        }
        assert(sent > 0);
    }
    if (send_timeout) {
        struct timeval timeout = {.tv_sec = 2, .tv_usec = 0};
        assert(setsockopt(sv[0], SOL_SOCKET, SO_SNDTIMEO, &timeout,
                          sizeof(timeout)) == 0);
    }

    install_parent_signal_action(signo, restart);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, signo) == 0);
        if (!expect_eintr) {
            usleep(50000);
            ssize_t total = 0;
            for (;;) {
                ssize_t received = recvfrom(sv[1], chunk, sizeof(chunk),
                                            MSG_DONTWAIT, NULL, NULL);
                if (received < 0) {
                    assert(errno == EAGAIN || errno == EWOULDBLOCK);
                    break;
                }
                assert(received > 0);
                total += received;
            }
            assert(total > 0);
        }
        _exit(0);
    }

    errno = 0;
    ssize_t sent = sendto(sv[0], "s", 1, 0, NULL, 0);
    if (expect_eintr)
        assert(sent == -1 && errno == EINTR);
    else
        assert(sent == 1);
    assert(signal_seen == expect_handler);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

static void test_recvmsg_signal(int signo, int restart, int expect_eintr,
                                int expect_handler)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    install_parent_signal_action(signo, restart);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, signo) == 0);
        if (!expect_eintr) {
            usleep(50000);
            assert(sendto(sv[1], "mv", 2, 0, NULL, 0) == 2);
        }
        _exit(0);
    }

    char first = 0;
    char second = 0;
    struct iovec iovs[2] = {
        {.iov_base = &first, .iov_len = 1},
        {.iov_base = &second, .iov_len = 1},
    };
    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = iovs;
    msg.msg_iovlen = 2;
    errno = 0;
    ssize_t received = recvmsg(sv[0], &msg, 0);
    if (expect_eintr)
        assert(received == -1 && errno == EINTR);
    else
        assert(received == 2 && first == 'm' && second == 'v');
    assert(signal_seen == expect_handler);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

static void test_recvmsg_partial_signal(void)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    install_parent_signal_action(SIGUSR1, 1);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(sendto(sv[1], "p", 1, 0, NULL, 0) == 1);
        usleep(50000);
        assert(kill(parent, SIGUSR1) == 0);
        _exit(0);
    }

    char bytes[2] = {0};
    struct iovec iov = {.iov_base = bytes, .iov_len = sizeof(bytes)};
    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    assert(recvmsg(sv[0], &msg, MSG_WAITALL) == 1);
    assert(bytes[0] == 'p');
    assert(signal_seen == 1);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

static void test_sendmsg_signal(int signo, int restart, int expect_eintr,
                                int expect_handler)
{
    int sv[2];
    char chunk[4096] = {0};
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    for (;;) {
        ssize_t sent = sendto(sv[0], chunk, sizeof(chunk), MSG_DONTWAIT, NULL, 0);
        if (sent < 0) {
            assert(errno == EAGAIN || errno == EWOULDBLOCK);
            break;
        }
        assert(sent > 0);
    }
    install_parent_signal_action(signo, restart);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, signo) == 0);
        if (!expect_eintr) {
            usleep(50000);
            for (;;) {
                ssize_t received = recvfrom(sv[1], chunk, sizeof(chunk),
                                            MSG_DONTWAIT, NULL, NULL);
                if (received < 0) {
                    assert(errno == EAGAIN || errno == EWOULDBLOCK);
                    break;
                }
                assert(received > 0);
            }
        }
        _exit(0);
    }

    char first = 'm';
    char second = 's';
    struct iovec iovs[2] = {
        {.iov_base = &first, .iov_len = 1},
        {.iov_base = &second, .iov_len = 1},
    };
    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = iovs;
    msg.msg_iovlen = 2;
    errno = 0;
    ssize_t sent = sendmsg(sv[0], &msg, 0);
    if (expect_eintr)
        assert(sent == -1 && errno == EINTR);
    else
        assert(sent == 2);
    assert(signal_seen == expect_handler);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

static void test_recvmmsg_signal(int signo, int restart, int expect_eintr,
                                 int expect_handler)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    install_parent_signal_action(signo, restart);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, signo) == 0);
        if (!expect_eintr) {
            usleep(50000);
            assert(sendto(sv[1], "mm", 2, 0, NULL, 0) == 2);
        }
        _exit(0);
    }

    char bytes[2] = {0};
    struct iovec iov = {.iov_base = bytes, .iov_len = sizeof(bytes)};
    struct mmsghdr message;
    memset(&message, 0, sizeof(message));
    message.msg_hdr.msg_iov = &iov;
    message.msg_hdr.msg_iovlen = 1;
    errno = 0;
    int received = recvmmsg(sv[0], &message, 1, 0, NULL);
    if (expect_eintr) {
        assert(received == -1 && errno == EINTR);
        assert(message.msg_len == 0);
    } else {
        assert(received == 1 && message.msg_len == 2);
        assert(memcmp(bytes, "mm", 2) == 0);
    }
    assert(signal_seen == expect_handler);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

static void test_recvmmsg_partial_signal(void)
{
    int sv[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    install_parent_signal_action(SIGUSR1, 1);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(sendto(sv[1], "p", 1, 0, NULL, 0) == 1);
        usleep(50000);
        assert(kill(parent, SIGUSR1) == 0);
        _exit(0);
    }

    char first = 0;
    char second = 0;
    struct iovec iovs[2] = {
        {.iov_base = &first, .iov_len = 1},
        {.iov_base = &second, .iov_len = 1},
    };
    struct mmsghdr messages[2];
    memset(messages, 0, sizeof(messages));
    messages[0].msg_hdr.msg_iov = &iovs[0];
    messages[0].msg_hdr.msg_iovlen = 1;
    messages[1].msg_hdr.msg_iov = &iovs[1];
    messages[1].msg_hdr.msg_iovlen = 1;
    assert(recvmmsg(sv[0], messages, 2, 0, NULL) == 1);
    assert(messages[0].msg_len == 1 && messages[1].msg_len == 0);
    assert(first == 'p');
    assert(signal_seen == 1);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

static long timespec_ms(const struct timespec *value)
{
    return value->tv_sec * 1000L + value->tv_nsec / 1000000L;
}

static void test_recvmmsg_timeout_modes(void)
{
    int sv[2];
    char byte = 0;
    struct iovec iov = {.iov_base = &byte, .iov_len = 1};
    struct mmsghdr message;
    memset(&message, 0, sizeof(message));
    message.msg_hdr.msg_iov = &iov;
    message.msg_hdr.msg_iovlen = 1;

    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(300000);
        assert(send(sv[1], "t", 1, 0) == 1);
        _exit(0);
    }
    struct timespec timeout = {.tv_sec = 0, .tv_nsec = 200000000};
    long started = monotonic_ms();
    errno = 0;
    int result = recvmmsg(sv[0], &message, 1, 0, &timeout);
    long elapsed = monotonic_ms() - started;
    assert(result == 1 && message.msg_len == 1 && byte == 't');
    assert(elapsed >= 250 && elapsed < 1000);
    assert(timeout.tv_sec == 0 && timeout.tv_nsec == 0);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);

    {
        assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
        assert(send(sv[1], "w", 1, 0) == 1);
        char wait_bytes[2] = {0};
        struct iovec wait_iovs[2] = {
            {.iov_base = &wait_bytes[0], .iov_len = 1},
            {.iov_base = &wait_bytes[1], .iov_len = 1},
        };
        struct mmsghdr wait_messages[2];
        memset(wait_messages, 0, sizeof(wait_messages));
        wait_messages[0].msg_hdr.msg_iov = &wait_iovs[0];
        wait_messages[0].msg_hdr.msg_iovlen = 1;
        wait_messages[1].msg_hdr.msg_iov = &wait_iovs[1];
        wait_messages[1].msg_hdr.msg_iovlen = 1;
        struct timespec wait_timeout = {
            .tv_sec = 0,
            .tv_nsec = 400000000,
        };
        long wait_started = monotonic_ms();
        assert(recvmmsg(sv[0], wait_messages, 2, MSG_WAITFORONE,
                        &wait_timeout) == 1);
        assert(monotonic_ms() - wait_started < 100);
        assert(wait_messages[0].msg_len == 1);
        assert(wait_messages[1].msg_len == 0);
        assert(wait_bytes[0] == 'w' && wait_bytes[1] == 0);
        assert(timespec_ms(&wait_timeout) > 300);
        assert(timespec_ms(&wait_timeout) <= 400);
        assert(close(sv[0]) == 0);
        assert(close(sv[1]) == 0);
    }

    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    install_parent_signal_action(SIGUSR1, 0);
    signal_seen = 0;
    child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, SIGUSR1) == 0);
        _exit(0);
    }
    memset(&message, 0, sizeof(message));
    message.msg_hdr.msg_iov = &iov;
    message.msg_hdr.msg_iovlen = 1;
    timeout = (struct timespec){.tv_sec = 0, .tv_nsec = 400000000};
    started = monotonic_ms();
    errno = 0;
    result = recvmmsg(sv[0], &message, 1, 0, &timeout);
    elapsed = monotonic_ms() - started;
    assert(result == -1 && errno == EINTR && elapsed < 300);
    assert(timeout.tv_sec == 0 && timeout.tv_nsec == 400000000);
    assert(signal_seen == 1);
    assert(waitpid(child, &status, 0) == child);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);

    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    install_parent_signal_action(SIGUSR1, 1);
    signal_seen = 0;
    child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, SIGUSR1) == 0);
        usleep(50000);
        assert(send(sv[1], "d", 1, 0) == 1);
        _exit(0);
    }
    memset(&message, 0, sizeof(message));
    message.msg_hdr.msg_iov = &iov;
    message.msg_hdr.msg_iovlen = 1;
    timeout = (struct timespec){.tv_sec = 0, .tv_nsec = 400000000};
    byte = 0;
    started = monotonic_ms();
    errno = 0;
    result = recvmmsg(sv[0], &message, 1, 0, &timeout);
    elapsed = monotonic_ms() - started;
    assert(result == 1 && message.msg_len == 1 && byte == 'd');
    assert(elapsed >= 70 && elapsed < 300);
    assert(timespec_ms(&timeout) > 250 && timespec_ms(&timeout) <= 400);
    assert(signal_seen == 1);
    assert(waitpid(child, &status, 0) == child);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);

    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    install_parent_signal_action(SIGUSR1, 1);
    signal_seen = 0;
    assert(send(sv[1], "p", 1, 0) == 1);
    child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, SIGUSR1) == 0);
        _exit(0);
    }
    char bytes[2] = {0};
    struct iovec iovs[2] = {
        {.iov_base = &bytes[0], .iov_len = 1},
        {.iov_base = &bytes[1], .iov_len = 1},
    };
    struct mmsghdr messages[2];
    memset(messages, 0, sizeof(messages));
    messages[0].msg_hdr.msg_iov = &iovs[0];
    messages[0].msg_hdr.msg_iovlen = 1;
    messages[1].msg_hdr.msg_iov = &iovs[1];
    messages[1].msg_hdr.msg_iovlen = 1;
    timeout = (struct timespec){.tv_sec = 0, .tv_nsec = 400000000};
    started = monotonic_ms();
    errno = 0;
    result = recvmmsg(sv[0], messages, 2, 0, &timeout);
    elapsed = monotonic_ms() - started;
    assert(result == 1 && elapsed < 300);
    assert(messages[0].msg_len == 1 && messages[1].msg_len == 0);
    assert(bytes[0] == 'p' && bytes[1] == 0);
    assert(timespec_ms(&timeout) > 300 && timespec_ms(&timeout) <= 400);
    assert(signal_seen == 1);
    assert(waitpid(child, &status, 0) == child);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);

    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    signal_seen = 0;
    child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, SIGWINCH) == 0);
        usleep(50000);
        assert(send(sv[1], "i", 1, 0) == 1);
        _exit(0);
    }
    byte = 0;
    memset(&message, 0, sizeof(message));
    message.msg_hdr.msg_iov = &iov;
    message.msg_hdr.msg_iovlen = 1;
    timeout = (struct timespec){.tv_sec = 0, .tv_nsec = 400000000};
    started = monotonic_ms();
    result = recvmmsg(sv[0], &message, 1, 0, &timeout);
    elapsed = monotonic_ms() - started;
    assert(result == 1 && message.msg_len == 1 && byte == 'i');
    assert(elapsed >= 70 && elapsed < 300);
    assert(timespec_ms(&timeout) > 200 && timespec_ms(&timeout) < 400);
    assert(signal_seen == 0);
    assert(waitpid(child, &status, 0) == child);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
    puts("SOCKET_PHASE5_LINUX recvmmsg_timeout_modes PASS");
}

static void test_sendmmsg_signal(int signo, int restart, int expect_eintr,
                                 int expect_handler)
{
    int sv[2];
    char chunk[4096] = {0};
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
    for (;;) {
        ssize_t sent = sendto(sv[0], chunk, sizeof(chunk), MSG_DONTWAIT, NULL, 0);
        if (sent < 0) {
            assert(errno == EAGAIN || errno == EWOULDBLOCK);
            break;
        }
        assert(sent > 0);
    }
    install_parent_signal_action(signo, restart);
    signal_seen = 0;
    pid_t parent = getpid();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(50000);
        assert(kill(parent, signo) == 0);
        if (!expect_eintr) {
            usleep(50000);
            for (;;) {
                ssize_t received = recvfrom(sv[1], chunk, sizeof(chunk),
                                            MSG_DONTWAIT, NULL, NULL);
                if (received < 0) {
                    assert(errno == EAGAIN || errno == EWOULDBLOCK);
                    break;
                }
                assert(received > 0);
            }
        }
        _exit(0);
    }

    char first = 'm';
    char second = 'm';
    struct iovec iovs[2] = {
        {.iov_base = &first, .iov_len = 1},
        {.iov_base = &second, .iov_len = 1},
    };
    struct mmsghdr message;
    memset(&message, 0, sizeof(message));
    message.msg_hdr.msg_iov = iovs;
    message.msg_hdr.msg_iovlen = 2;
    errno = 0;
    int sent = sendmmsg(sv[0], &message, 1, 0);
    if (expect_eintr) {
        assert(sent == -1 && errno == EINTR);
        assert(message.msg_len == 0);
    } else {
        assert(sent == 1 && message.msg_len == 2);
    }
    assert(signal_seen == expect_handler);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(sv[0]) == 0);
    assert(close(sv[1]) == 0);
}

int main(void)
{
    setbuf(stdout, NULL);
    assert(signal(SIGPIPE, SIG_IGN) != SIG_ERR);

    char directory[] = "/tmp/respos-socket-phase5-XXXXXX";
    assert(mkdtemp(directory) != NULL);
    char path_one[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_two[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_three[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_four[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_five[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_six[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_seven[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_eight[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_nine[sizeof(((struct sockaddr_un *)0)->sun_path)];
    assert(snprintf(path_one, sizeof(path_one), "%s/path.sock", directory) > 0);
    assert(snprintf(path_two, sizeof(path_two), "%s/eintr.sock", directory) > 0);
    assert(snprintf(path_three, sizeof(path_three), "%s/restart.sock", directory) > 0);
    assert(snprintf(path_four, sizeof(path_four), "%s/ignored.sock", directory) > 0);
    assert(snprintf(path_five, sizeof(path_five), "%s/connect-eintr.sock", directory) > 0);
    assert(snprintf(path_six, sizeof(path_six), "%s/connect-restart.sock", directory) > 0);
    assert(snprintf(path_seven, sizeof(path_seven), "%s/connect-ignored.sock", directory) > 0);
    assert(snprintf(path_eight, sizeof(path_eight), "%s/connect-timeout.sock", directory) > 0);
    assert(snprintf(path_nine, sizeof(path_nine), "%s/accept-timeout.sock", directory) > 0);

    test_pathname_and_nonblock(path_one);
    test_shutdown_and_poll();
    test_rdhup_blocking_and_epoll_modes();
    test_blocking_poll_and_pipe_events();
    test_accept_signal(path_two, SIGUSR1, 0, 0, 1, 1);
    test_accept_signal(path_three, SIGUSR1, 1, 0, 0, 1);
    test_accept_signal(path_four, SIGWINCH, 0, 0, 0, 0);
    test_accept_signal(path_nine, SIGUSR1, 1, 1, 1, 1);
    puts("SOCKET_PHASE5_LINUX accept_restart_modes PASS");
    test_connect_signal(path_five, SIGUSR1, 0, 0, 1, 1);
    test_connect_signal(path_six, SIGUSR1, 1, 0, 0, 1);
    test_connect_signal(path_seven, SIGWINCH, 0, 0, 0, 0);
    test_connect_signal(path_eight, SIGUSR1, 1, 1, 1, 1);
    puts("SOCKET_PHASE5_LINUX connect_restart_modes PASS");
    test_recvfrom_signal(SIGUSR1, 0, 0, 1, 1);
    test_sendto_signal(SIGUSR1, 0, 0, 1, 1);
    test_recvfrom_signal(SIGUSR1, 1, 0, 0, 1);
    test_sendto_signal(SIGUSR1, 1, 0, 0, 1);
    test_recvfrom_signal(SIGWINCH, 0, 0, 0, 0);
    test_sendto_signal(SIGWINCH, 0, 0, 0, 0);
    test_recvfrom_signal(SIGUSR1, 1, 1, 1, 1);
    test_sendto_signal(SIGUSR1, 1, 1, 1, 1);
    puts("SOCKET_PHASE5_LINUX send_recv_restart_modes PASS");
    test_recvmsg_signal(SIGUSR1, 0, 1, 1);
    test_sendmsg_signal(SIGUSR1, 0, 1, 1);
    test_recvmsg_signal(SIGUSR1, 1, 0, 1);
    test_sendmsg_signal(SIGUSR1, 1, 0, 1);
    test_recvmsg_partial_signal();
    test_recvmsg_signal(SIGWINCH, 0, 0, 0);
    test_sendmsg_signal(SIGWINCH, 0, 0, 0);
    puts("SOCKET_PHASE5_LINUX msg_restart_modes PASS");
    test_recvmmsg_signal(SIGUSR1, 0, 1, 1);
    test_sendmmsg_signal(SIGUSR1, 0, 1, 1);
    test_recvmmsg_signal(SIGUSR1, 1, 0, 1);
    test_sendmmsg_signal(SIGUSR1, 1, 0, 1);
    test_recvmmsg_partial_signal();
    test_recvmmsg_signal(SIGWINCH, 0, 0, 0);
    test_sendmmsg_signal(SIGWINCH, 0, 0, 0);
    puts("SOCKET_PHASE5_LINUX mmsg_restart_modes PASS");
    test_recvmmsg_timeout_modes();

    assert(unlink(path_one) == 0);
    assert(unlink(path_two) == 0);
    assert(unlink(path_three) == 0);
    assert(unlink(path_four) == 0);
    assert(unlink(path_five) == 0);
    assert(unlink(path_six) == 0);
    assert(unlink(path_seven) == 0);
    assert(unlink(path_eight) == 0);
    assert(unlink(path_nine) == 0);
    assert(rmdir(directory) == 0);
    puts("SOCKET_PHASE5_LINUX ALL PASS");
    return 0;
}
