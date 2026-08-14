#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <unistd.h>

static void fail(const char *what)
{
    perror(what);
    exit(1);
}

static void expect_errno(ssize_t result, int expected, const char *what)
{
    if (result != -1 || errno != expected) {
        fprintf(stderr, "FAIL: %s result=%zd errno=%d expected=%d\n",
                what, result, errno, expected);
        exit(1);
    }
}

static void test_unconnected_socket_errors(void)
{
    int pipefd[2];
    int unix_fd;
    int inet_fd;

    if (pipe(pipefd) < 0)
        fail("pipe");
    unix_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (unix_fd < 0)
        fail("socket(AF_UNIX)");
    inet_fd = socket(AF_INET, SOCK_STREAM, 0);
    if (inet_fd < 0)
        fail("socket(AF_INET)");

    errno = 0;
    expect_errno(splice(unix_fd, NULL, pipefd[1], NULL, 1, 0),
                 EINVAL, "unconnected unix socket to pipe write end");
    errno = 0;
    expect_errno(splice(inet_fd, NULL, pipefd[1], NULL, 1, 0),
                 ENOTCONN, "unconnected inet socket to pipe write end");
    errno = 0;
    expect_errno(splice(unix_fd, NULL, pipefd[0], NULL, 1, 0),
                 EBADF, "unix socket to pipe read end");

    close(unix_fd);
    close(inet_fd);
    close(pipefd[0]);
    close(pipefd[1]);
}

static void test_connected_unix_socket_to_pipe(void)
{
    int pipefd[2];
    int sv[2];
    char byte = 0;

    if (pipe(pipefd) < 0)
        fail("pipe(connected)");
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair");
    if (write(sv[1], "x", 1) != 1)
        fail("write(socketpair)");
    if (splice(sv[0], NULL, pipefd[1], NULL, 1, 0) != 1)
        fail("splice(connected unix socket)");
    if (read(pipefd[0], &byte, 1) != 1 || byte != 'x') {
        fprintf(stderr, "FAIL: connected splice payload=%d\n", byte);
        exit(1);
    }

    close(sv[0]);
    close(sv[1]);
    close(pipefd[0]);
    close(pipefd[1]);
}

int main(void)
{
    test_unconnected_socket_errors();
    test_connected_unix_socket_to_pipe();
    puts("splice socket Linux probe: PASS");
    return 0;
}
