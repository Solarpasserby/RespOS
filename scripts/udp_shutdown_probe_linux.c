#define _GNU_SOURCE

#include <arpa/inet.h>
#include <assert.h>
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

struct udp_pair {
    int left;
    int right;
};

static void bind_loopback(int fd, struct sockaddr_in *addr)
{
    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr->sin_port = 0;
    assert(bind(fd, (const struct sockaddr *)addr, sizeof(*addr)) == 0);
    socklen_t addrlen = sizeof(*addr);
    assert(getsockname(fd, (struct sockaddr *)addr, &addrlen) == 0);
    assert(addrlen == sizeof(*addr));
    assert(addr->sin_port != 0);
}

static struct udp_pair make_pair(void)
{
    struct udp_pair pair = {
        .left = socket(AF_INET, SOCK_DGRAM, 0),
        .right = socket(AF_INET, SOCK_DGRAM, 0),
    };
    assert(pair.left >= 0 && pair.right >= 0);
    struct sockaddr_in left_addr;
    struct sockaddr_in right_addr;
    bind_loopback(pair.left, &left_addr);
    bind_loopback(pair.right, &right_addr);
    assert(connect(pair.left, (const struct sockaddr *)&right_addr,
                   sizeof(right_addr)) == 0);
    assert(connect(pair.right, (const struct sockaddr *)&left_addr,
                   sizeof(left_addr)) == 0);
    return pair;
}

static void close_pair(struct udp_pair pair)
{
    assert(close(pair.left) == 0);
    assert(close(pair.right) == 0);
}

static void send_and_receive(int sender, int receiver, const char *payload)
{
    size_t length = strlen(payload);
    assert(send(sender, payload, length, 0) == (ssize_t)length);
    char buffer[16] = {0};
    assert(length <= sizeof(buffer));
    assert(recv(receiver, buffer, sizeof(buffer), 0) == (ssize_t)length);
    assert(memcmp(buffer, payload, length) == 0);
}

static void expect_send_epipe(int fd)
{
    errno = 0;
    assert(send(fd, "x", 1, MSG_NOSIGNAL) == -1);
    assert(errno == EPIPE);
}

static void expect_readiness(int fd, short expected)
{
    const short interest = POLLIN | POLLOUT | POLLRDHUP;
    struct pollfd pollfd = {.fd = fd, .events = interest};
    assert(poll(&pollfd, 1, 0) == 1);
    assert(pollfd.revents == expected);

    int epfd = epoll_create1(0);
    assert(epfd >= 0);
    struct epoll_event event = {
        .events = EPOLLIN | EPOLLOUT | EPOLLRDHUP,
        .data.u64 = 0x55445053485554ULL,
    };
    assert(epoll_ctl(epfd, EPOLL_CTL_ADD, fd, &event) == 0);
    memset(&event, 0, sizeof(event));
    assert(epoll_wait(epfd, &event, 1, 0) == 1);
    assert(event.events == (uint32_t)expected);
    assert(event.data.u64 == 0x55445053485554ULL);
    assert(close(epfd) == 0);
}

static void expect_blocking_poll_shutdown(void)
{
    struct udp_pair pair = make_pair();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(100000);
        _exit(shutdown(pair.left, SHUT_RD) == 0 ? 0 : 1);
    }

    struct pollfd pollfd = {
        .fd = pair.left,
        .events = POLLIN | POLLRDHUP,
    };
    assert(poll(&pollfd, 1, 2000) == 1);
    assert(pollfd.revents == (POLLIN | POLLRDHUP));
    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    close_pair(pair);

    pair = make_pair();
    int epfd = epoll_create1(0);
    assert(epfd >= 0);
    struct epoll_event event = {
        .events = EPOLLIN | EPOLLRDHUP,
        .data.u64 = 0x554450424c4f43ULL,
    };
    assert(epoll_ctl(epfd, EPOLL_CTL_ADD, pair.left, &event) == 0);
    child = fork();
    assert(child >= 0);
    if (child == 0) {
        usleep(100000);
        _exit(shutdown(pair.left, SHUT_RD) == 0 ? 0 : 1);
    }
    memset(&event, 0, sizeof(event));
    assert(epoll_wait(epfd, &event, 1, 2000) == 1);
    assert(event.events == (EPOLLIN | EPOLLRDHUP));
    assert(event.data.u64 == 0x554450424c4f43ULL);
    status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(close(epfd) == 0);
    close_pair(pair);
}

int main(void)
{
    setbuf(stdout, NULL);
    alarm(10);
    assert(signal(SIGPIPE, SIG_IGN) != SIG_ERR);

    int unconnected = socket(AF_INET, SOCK_DGRAM, 0);
    assert(unconnected >= 0);
    for (int how = SHUT_RD; how <= SHUT_RDWR; ++how) {
        errno = 0;
        assert(shutdown(unconnected, how) == -1 && errno == ENOTCONN);
    }
    assert(close(unconnected) == 0);

    int bound = socket(AF_INET, SOCK_DGRAM, 0);
    assert(bound >= 0);
    struct sockaddr_in bound_addr;
    bind_loopback(bound, &bound_addr);
    errno = 0;
    assert(shutdown(bound, SHUT_RDWR) == -1 && errno == ENOTCONN);
    assert(close(bound) == 0);

    struct udp_pair pair = make_pair();
    errno = 0;
    assert(shutdown(pair.left, 3) == -1 && errno == EINVAL);
    send_and_receive(pair.left, pair.right, "before");
    assert(shutdown(pair.left, SHUT_WR) == 0);
    assert(shutdown(pair.left, SHUT_WR) == 0);
    expect_readiness(pair.left, POLLOUT);
    expect_send_epipe(pair.left);
    send_and_receive(pair.right, pair.left, "reverse");
    close_pair(pair);

    pair = make_pair();
    assert(shutdown(pair.left, SHUT_RD) == 0);
    expect_readiness(pair.left, POLLIN | POLLOUT | POLLRDHUP);
    close_pair(pair);

    pair = make_pair();
    assert(send(pair.right, "drop", 4, 0) == 4);
    assert(shutdown(pair.left, SHUT_RD) == 0);
    char byte = 0;
    ssize_t after_shut_rd = recv(pair.left, &byte, 1, 0);
    assert(after_shut_rd == 1 && byte == 'd');
    assert(recv(pair.left, &byte, 1, 0) == 0);
    send_and_receive(pair.left, pair.right, "out");
    send_and_receive(pair.right, pair.left, "future");
    assert(recv(pair.left, &byte, 1, 0) == 0);
    close_pair(pair);

    pair = make_pair();
    assert(shutdown(pair.left, SHUT_RDWR) == 0);
    expect_readiness(pair.left, POLLIN | POLLOUT | POLLHUP | POLLRDHUP);
    assert(recv(pair.left, &byte, 1, 0) == 0);
    expect_send_epipe(pair.left);
    close_pair(pair);

    expect_blocking_poll_shutdown();

    alarm(0);
    puts("UDP_SHUTDOWN_LINUX PASS unconnected=pass shut_wr=pass shut_rd=pass shut_rdwr=pass readiness=pass blocking_poll=pass blocking_epoll=pass");
    return 0;
}
