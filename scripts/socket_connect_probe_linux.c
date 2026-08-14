#define _GNU_SOURCE

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

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

static void make_nonblocking(int fd)
{
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0)
        fail("fcntl(O_NONBLOCK)");
}

static int read_so_error(int fd)
{
    int error = -1;
    socklen_t len = sizeof(error);

    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &len) < 0)
        fail("getsockopt(SO_ERROR)");
    expect(len == sizeof(error), "SO_ERROR optlen");
    return error;
}

static void wait_connect_ready(int fd, short *revents)
{
    struct pollfd pfd = {.fd = fd, .events = POLLOUT};
    int ready = poll(&pfd, 1, 1000);

    if (ready < 0)
        fail("poll(connect)");
    expect(ready == 1, "connect becomes poll-ready");
    expect((pfd.revents & POLLOUT) != 0, "connect reports POLLOUT");
    *revents = pfd.revents;
}

static void loopback_addr(struct sockaddr_in *addr, uint16_t port)
{
    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_port = htons(port);
    addr->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
}

static void test_success(void)
{
    struct sockaddr_in addr;
    socklen_t addrlen = sizeof(addr);
    short revents = 0;
    int listener = socket(AF_INET, SOCK_STREAM, 0);
    int client;
    int accepted;

    if (listener < 0)
        fail("socket(listener)");
    loopback_addr(&addr, 0);
    if (bind(listener, (struct sockaddr *)&addr, sizeof(addr)) < 0)
        fail("bind(listener)");
    if (getsockname(listener, (struct sockaddr *)&addr, &addrlen) < 0)
        fail("getsockname(listener)");
    if (listen(listener, 4) < 0)
        fail("listen(listener)");

    client = socket(AF_INET, SOCK_STREAM, 0);
    if (client < 0)
        fail("socket(success client)");
    make_nonblocking(client);
    errno = 0;
    expect(connect(client, (struct sockaddr *)&addr, sizeof(addr)) == -1
               && errno == EINPROGRESS,
           "first nonblocking connect returns EINPROGRESS");
    wait_connect_ready(client, &revents);
    expect((revents & POLLERR) == 0, "successful connect has no POLLERR");
    expect(read_so_error(client) == 0, "successful connect SO_ERROR is zero");
    expect(read_so_error(client) == 0, "successful SO_ERROR remains zero");

    accepted = accept(listener, NULL, NULL);
    if (accepted < 0)
        fail("accept(success)");
    if (send(client, "s", 1, 0) != 1)
        fail("send(success)");
    {
        char byte = 0;
        expect(recv(accepted, &byte, 1, 0) == 1 && byte == 's',
               "connected socket transfers data");
    }
    close(accepted);
    close(client);
    close(listener);
}

static void test_refused_and_error_consumption(void)
{
    struct sockaddr_in addr;
    socklen_t addrlen = sizeof(addr);
    short revents = 0;
    int reservation = socket(AF_INET, SOCK_STREAM, 0);
    int client;

    if (reservation < 0)
        fail("socket(port reservation)");
    loopback_addr(&addr, 0);
    if (bind(reservation, (struct sockaddr *)&addr, sizeof(addr)) < 0)
        fail("bind(port reservation)");
    if (getsockname(reservation, (struct sockaddr *)&addr, &addrlen) < 0)
        fail("getsockname(port reservation)");
    close(reservation);

    client = socket(AF_INET, SOCK_STREAM, 0);
    if (client < 0)
        fail("socket(refused client)");
    make_nonblocking(client);
    errno = 0;
    expect(connect(client, (struct sockaddr *)&addr, sizeof(addr)) == -1
               && errno == EINPROGRESS,
           "refused nonblocking connect first returns EINPROGRESS");
    wait_connect_ready(client, &revents);
    expect((revents & POLLERR) != 0, "refused connect reports POLLERR");
    expect(read_so_error(client) == ECONNREFUSED,
           "SO_ERROR exposes ECONNREFUSED");
    expect(read_so_error(client) == 0, "SO_ERROR consumes pending error");
    wait_connect_ready(client, &revents);
    expect((revents & POLLERR) == 0,
           "consumed SO_ERROR clears exceptional readiness");
    close(client);
}

static void test_blocking_refused_has_no_pending_error(void)
{
    struct sockaddr_in addr;
    socklen_t addrlen = sizeof(addr);
    int reservation = socket(AF_INET, SOCK_STREAM, 0);
    int client;

    if (reservation < 0)
        fail("socket(blocking port reservation)");
    loopback_addr(&addr, 0);
    if (bind(reservation, (struct sockaddr *)&addr, sizeof(addr)) < 0)
        fail("bind(blocking port reservation)");
    if (getsockname(reservation, (struct sockaddr *)&addr, &addrlen) < 0)
        fail("getsockname(blocking port reservation)");
    close(reservation);

    client = socket(AF_INET, SOCK_STREAM, 0);
    if (client < 0)
        fail("socket(blocking refused client)");
    errno = 0;
    expect(connect(client, (struct sockaddr *)&addr, sizeof(addr)) == -1
               && errno == ECONNREFUSED,
           "blocking refused connect returns ECONNREFUSED");
    expect(read_so_error(client) == 0,
           "blocking connect failure leaves no pending SO_ERROR");
    close(client);
}

static void test_retry_after_refused_error_consumption(void)
{
    struct sockaddr_in addr;
    socklen_t addrlen = sizeof(addr);
    short revents = 0;
    int reservation = socket(AF_INET, SOCK_STREAM, 0);
    int listener;
    int client;
    int accepted;

    if (reservation < 0)
        fail("socket(retry port reservation)");
    loopback_addr(&addr, 0);
    if (bind(reservation, (struct sockaddr *)&addr, sizeof(addr)) < 0)
        fail("bind(retry port reservation)");
    if (getsockname(reservation, (struct sockaddr *)&addr, &addrlen) < 0)
        fail("getsockname(retry port reservation)");
    close(reservation);

    client = socket(AF_INET, SOCK_STREAM, 0);
    if (client < 0)
        fail("socket(retry client)");
    make_nonblocking(client);
    errno = 0;
    expect(connect(client, (struct sockaddr *)&addr, sizeof(addr)) == -1
               && errno == EINPROGRESS,
           "retry setup first connect returns EINPROGRESS");
    wait_connect_ready(client, &revents);
    expect((revents & POLLERR) != 0,
           "retry setup refused connect reports POLLERR");
    expect(read_so_error(client) == ECONNREFUSED,
           "retry setup consumes ECONNREFUSED");

    listener = socket(AF_INET, SOCK_STREAM, 0);
    if (listener < 0)
        fail("socket(retry listener)");
    if (bind(listener, (struct sockaddr *)&addr, sizeof(addr)) < 0)
        fail("bind(retry listener)");
    if (listen(listener, 4) < 0)
        fail("listen(retry listener)");

    errno = 0;
    expect(connect(client, (struct sockaddr *)&addr, sizeof(addr)) == -1
               && errno == ECONNABORTED,
           "first connect after consumed failure resets the socket");
    errno = 0;
    expect(connect(client, (struct sockaddr *)&addr, sizeof(addr)) == -1
               && errno == EINPROGRESS,
           "second connect after reset starts a new attempt");
    wait_connect_ready(client, &revents);
    expect((revents & POLLERR) == 0,
           "retried successful connect has no POLLERR");
    expect(read_so_error(client) == 0,
           "retried successful connect has zero SO_ERROR");

    accepted = accept(listener, NULL, NULL);
    if (accepted < 0)
        fail("accept(retry)");
    if (send(client, "r", 1, 0) != 1)
        fail("send(retry)");
    {
        char byte = 0;
        expect(recv(accepted, &byte, 1, 0) == 1 && byte == 'r',
               "retried socket transfers data");
    }
    close(accepted);
    close(client);
    close(listener);
}

int main(void)
{
    test_success();
    test_refused_and_error_consumption();
    test_blocking_refused_has_no_pending_error();
    test_retry_after_refused_error_consumption();
    puts("socket connect Linux probe: PASS");
    return 0;
}
