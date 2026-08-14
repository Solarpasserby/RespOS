#define _GNU_SOURCE

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <unistd.h>

static void fail(const char *what)
{
    perror(what);
    exit(1);
}

static void expect_errno(int result, int expected, const char *what)
{
    if (result != -1 || errno != expected) {
        fprintf(stderr, "FAIL: %s result=%d errno=%d expected=%d\n",
                what, result, errno, expected);
        exit(1);
    }
}

static void test_descriptor_and_connection_errors(void)
{
    struct sockaddr_in addr = {0};
    socklen_t addrlen = sizeof(addr);
    int fd;

    errno = 0;
    expect_errno(getpeername(-1, (struct sockaddr *)&addr, &addrlen),
                 EBADF, "invalid descriptor");

    fd = open("/dev/null", O_WRONLY);
    if (fd < 0)
        fail("open(/dev/null)");
    errno = 0;
    expect_errno(getpeername(fd, (struct sockaddr *)&addr, &addrlen),
                 ENOTSOCK, "non-socket descriptor");
    close(fd);

    fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0)
        fail("socket(unconnected)");
    addr.sin_family = AF_INET;
    addr.sin_port = 0;
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0)
        fail("bind(unconnected)");
    addrlen = sizeof(addr);
    errno = 0;
    expect_errno(getpeername(fd, (struct sockaddr *)&addr, &addrlen),
                 ENOTCONN, "unconnected socket");
    addrlen = (socklen_t)-1;
    errno = 0;
    expect_errno(getpeername(fd, (struct sockaddr *)&addr, &addrlen),
                 ENOTCONN, "unconnected socket precedes invalid addrlen");
    close(fd);
}

static void test_output_validation_precedes_peer_lookup(void)
{
    struct sockaddr_storage addr = {0};
    socklen_t addrlen;
    int sv[2];

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair");

    addrlen = (socklen_t)-1;
    errno = 0;
    expect_errno(getpeername(sv[0], (struct sockaddr *)&addr, &addrlen),
                 EINVAL, "negative addrlen");

    addrlen = sizeof(addr);
    errno = 0;
    expect_errno(getpeername(sv[0], (struct sockaddr *)(uintptr_t)-1,
                             &addrlen),
                 EFAULT, "invalid sockaddr pointer");

    errno = 0;
    expect_errno(getpeername(sv[0], (struct sockaddr *)&addr, NULL),
                 EFAULT, "null addrlen pointer");

    errno = 0;
    expect_errno(getpeername(sv[0], (struct sockaddr *)&addr,
                             (socklen_t *)(uintptr_t)1),
                 EFAULT, "invalid addrlen pointer");

    addrlen = sizeof(addr);
    if (getpeername(sv[0], (struct sockaddr *)&addr, &addrlen) < 0)
        fail("getpeername(socketpair)");
    if (((struct sockaddr *)&addr)->sa_family != AF_UNIX ||
        addrlen != sizeof(sa_family_t)) {
        fprintf(stderr, "FAIL: socketpair peer family=%d len=%u\n",
                ((struct sockaddr *)&addr)->sa_family, (unsigned)addrlen);
        exit(1);
    }

    close(sv[0]);
    close(sv[1]);
}

int main(void)
{
    test_descriptor_and_connection_errors();
    test_output_validation_precedes_peer_lookup();
    puts("getpeername Linux probe: PASS");
    return 0;
}
