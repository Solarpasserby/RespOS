#define _GNU_SOURCE

#include <arpa/inet.h>
#include <assert.h>
#include <errno.h>
#include <netinet/in.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int make_listener(struct sockaddr_in *addr)
{
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    assert(fd >= 0);

    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr->sin_port = 0;
    assert(bind(fd, (const struct sockaddr *)addr, sizeof(*addr)) == 0);
    assert(listen(fd, 4) == 0);

    socklen_t addrlen = sizeof(*addr);
    assert(getsockname(fd, (struct sockaddr *)addr, &addrlen) == 0);
    assert(addrlen == sizeof(*addr));
    assert(addr->sin_port != 0);
    return fd;
}

static void receive_exact(int fd, const char *expected)
{
    char buffer[16] = { 0 };
    size_t expected_len = strlen(expected);
    assert(expected_len <= sizeof(buffer));
    size_t received = 0;
    while (received < expected_len) {
        ssize_t result = recv(fd, buffer + received, expected_len - received, 0);
        assert(result > 0);
        received += (size_t)result;
    }
    assert(memcmp(buffer, expected, expected_len) == 0);
}

static void expect_readable(int fd)
{
    struct pollfd pollfd = { .fd = fd, .events = POLLIN };
    assert(poll(&pollfd, 1, 1000) == 1);
    assert((pollfd.revents & POLLIN) != 0);
}

int main(void)
{
    setbuf(stdout, NULL);
    assert(signal(SIGPIPE, SIG_IGN) != SIG_ERR);

    int unconnected = socket(AF_INET, SOCK_STREAM, 0);
    assert(unconnected >= 0);
    errno = 0;
    assert(shutdown(unconnected, SHUT_WR) == -1 && errno == ENOTCONN);
    assert(close(unconnected) == 0);

    struct sockaddr_in addr;
    int listener = make_listener(&addr);

    int client = socket(AF_INET, SOCK_STREAM, 0);
    assert(client >= 0);
    assert(connect(client, (const struct sockaddr *)&addr, sizeof(addr)) == 0);
    int server = accept(listener, NULL, NULL);
    assert(server >= 0);

    errno = 0;
    assert(shutdown(client, 3) == -1 && errno == EINVAL);
    assert(send(client, "request", 7, 0) == 7);

    int duplicate = dup(client);
    assert(duplicate >= 0);
    assert(shutdown(client, SHUT_WR) == 0);
    assert(shutdown(client, SHUT_WR) == 0);
    errno = 0;
    assert(send(duplicate, "x", 1, MSG_NOSIGNAL) == -1 && errno == EPIPE);

    receive_exact(server, "request");
    char byte = 0;
    expect_readable(server);
    assert(recv(server, &byte, 1, 0) == 0);
    assert(send(server, "response", 8, 0) == 8);
    receive_exact(duplicate, "response");

    assert(shutdown(server, SHUT_WR) == 0);
    expect_readable(client);
    assert(recv(client, &byte, 1, 0) == 0);

    assert(close(duplicate) == 0);
    assert(close(client) == 0);
    assert(close(server) == 0);
    assert(close(listener) == 0);
    puts("TCP_HALF_CLOSE_LINUX PASS errors=pass queued_fin=pass reverse_flow=pass dup=pass poll_eof=pass");
    return 0;
}
