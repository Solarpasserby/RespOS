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
#include <sys/wait.h>
#include <unistd.h>

static volatile sig_atomic_t signal_seen;

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

static void test_accept_eintr(const char *path)
{
    struct sockaddr_un addr;
    socklen_t addrlen = make_addr(&addr, path);
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    int ready[2];
    assert(listener >= 0);
    assert(bind(listener, (struct sockaddr *)&addr, addrlen) == 0);
    assert(listen(listener, 4) == 0);
    assert(pipe(ready) == 0);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        struct sigaction action;
        memset(&action, 0, sizeof(action));
        action.sa_handler = signal_handler;
        sigemptyset(&action.sa_mask);
        assert(sigaction(SIGUSR1, &action, NULL) == 0);
        assert(close(ready[0]) == 0);
        assert(write(ready[1], "r", 1) == 1);
        assert(close(ready[1]) == 0);

        errno = 0;
        assert(accept(listener, NULL, NULL) == -1);
        assert(errno == EINTR);
        assert(signal_seen == 1);
        _exit(0);
    }

    assert(close(ready[1]) == 0);
    char byte;
    assert(read(ready[0], &byte, 1) == 1);
    assert(close(ready[0]) == 0);
    usleep(50000);
    assert(kill(child, SIGUSR1) == 0);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(listener) == 0);
    puts("SOCKET_PHASE5_LINUX accept_eintr PASS");
}

int main(void)
{
    setbuf(stdout, NULL);
    assert(signal(SIGPIPE, SIG_IGN) != SIG_ERR);

    char directory[] = "/tmp/respos-socket-phase5-XXXXXX";
    assert(mkdtemp(directory) != NULL);
    char path_one[sizeof(((struct sockaddr_un *)0)->sun_path)];
    char path_two[sizeof(((struct sockaddr_un *)0)->sun_path)];
    assert(snprintf(path_one, sizeof(path_one), "%s/path.sock", directory) > 0);
    assert(snprintf(path_two, sizeof(path_two), "%s/eintr.sock", directory) > 0);

    test_pathname_and_nonblock(path_one);
    test_shutdown_and_poll();
    test_rdhup_blocking_and_epoll_modes();
    test_blocking_poll_and_pipe_events();
    test_accept_eintr(path_two);

    assert(unlink(path_one) == 0);
    assert(unlink(path_two) == 0);
    assert(rmdir(directory) == 0);
    puts("SOCKET_PHASE5_LINUX ALL PASS");
    return 0;
}
