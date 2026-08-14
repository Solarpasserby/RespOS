#define _GNU_SOURCE

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
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

static socklen_t make_path_addr(struct sockaddr_un *addr, const char *path)
{
    size_t len = strlen(path);
    memset(addr, 0, sizeof(*addr));
    addr->sun_family = AF_UNIX;
    if (len >= sizeof(addr->sun_path)) {
        fprintf(stderr, "FAIL: pathname too long\n");
        exit(1);
    }
    memcpy(addr->sun_path, path, len + 1);
    return (socklen_t)(offsetof(struct sockaddr_un, sun_path) + len + 1);
}

static socklen_t make_abstract_addr(struct sockaddr_un *addr,
                                    const unsigned char *name, size_t name_len)
{
    memset(addr, 0, sizeof(*addr));
    addr->sun_family = AF_UNIX;
    if (name_len + 1 > sizeof(addr->sun_path)) {
        fprintf(stderr, "FAIL: abstract name too long\n");
        exit(1);
    }
    addr->sun_path[0] = '\0';
    memcpy(addr->sun_path + 1, name, name_len);
    return (socklen_t)(offsetof(struct sockaddr_un, sun_path) + 1 + name_len);
}

static void expect_unix_addr(const struct sockaddr_un *actual, socklen_t actual_len,
                             const struct sockaddr_un *expected, socklen_t expected_len,
                             const char *what)
{
    if (actual_len != expected_len || actual->sun_family != AF_UNIX ||
        memcmp(actual, expected, expected_len) != 0) {
        fprintf(stderr, "FAIL: %s len=%u expected_len=%u\n", what,
                (unsigned)actual_len, (unsigned)expected_len);
        exit(1);
    }
}

static void test_named_unix_addresses(int abstract)
{
    static const unsigned char server_abstract[] = {'s', 'r', 'v', 0xfe, 'x'};
    static const unsigned char client_abstract[] = {'c', 'l', 'i', 0xfd, 'y'};
    struct sockaddr_un server_addr;
    struct sockaddr_un client_addr;
    char server_path[96];
    char client_path[96];
    socklen_t server_len;
    socklen_t client_len;
    int listener;
    int client;
    int accepted;

    if (abstract) {
        server_len = make_abstract_addr(&server_addr, server_abstract,
                                        sizeof(server_abstract));
        client_len = make_abstract_addr(&client_addr, client_abstract,
                                        sizeof(client_abstract));
    } else {
        snprintf(server_path, sizeof(server_path), "/tmp/respos-peer-s-%ld", (long)getpid());
        snprintf(client_path, sizeof(client_path), "/tmp/respos-peer-c-%ld", (long)getpid());
        unlink(server_path);
        unlink(client_path);
        server_len = make_path_addr(&server_addr, server_path);
        client_len = make_path_addr(&client_addr, client_path);
    }

    listener = socket(AF_UNIX, SOCK_STREAM, 0);
    client = socket(AF_UNIX, SOCK_STREAM, 0);
    if (listener < 0 || client < 0)
        fail("socket(named AF_UNIX)");
    if (bind(listener, (struct sockaddr *)&server_addr, server_len) < 0)
        fail("bind(server AF_UNIX)");
    if (bind(client, (struct sockaddr *)&client_addr, client_len) < 0)
        fail("bind(client AF_UNIX)");
    if (listen(listener, 1) < 0)
        fail("listen(AF_UNIX)");
    if (connect(client, (struct sockaddr *)&server_addr, server_len) < 0)
        fail("connect(AF_UNIX)");

    struct sockaddr_un observed = {0};
    socklen_t observed_len = sizeof(observed);
    accepted = accept(listener, (struct sockaddr *)&observed, &observed_len);
    if (accepted < 0)
        fail("accept(AF_UNIX)");
    expect_unix_addr(&observed, observed_len, &client_addr, client_len,
                     "accept peer address");

    observed_len = sizeof(observed);
    if (getsockname(listener, (struct sockaddr *)&observed, &observed_len) < 0)
        fail("getsockname(listener AF_UNIX)");
    expect_unix_addr(&observed, observed_len, &server_addr, server_len,
                     "listener local address");

    observed_len = sizeof(observed);
    if (getsockname(client, (struct sockaddr *)&observed, &observed_len) < 0)
        fail("getsockname(client AF_UNIX)");
    expect_unix_addr(&observed, observed_len, &client_addr, client_len,
                     "client local address");

    observed_len = sizeof(observed);
    if (getpeername(client, (struct sockaddr *)&observed, &observed_len) < 0)
        fail("getpeername(client AF_UNIX)");
    expect_unix_addr(&observed, observed_len, &server_addr, server_len,
                     "client peer address");

    observed_len = sizeof(observed);
    if (getsockname(accepted, (struct sockaddr *)&observed, &observed_len) < 0)
        fail("getsockname(accepted AF_UNIX)");
    expect_unix_addr(&observed, observed_len, &server_addr, server_len,
                     "accepted local address");

    observed_len = sizeof(observed);
    if (getpeername(accepted, (struct sockaddr *)&observed, &observed_len) < 0)
        fail("getpeername(accepted AF_UNIX)");
    expect_unix_addr(&observed, observed_len, &client_addr, client_len,
                     "accepted peer address");

    unsigned char truncated[4] = {0xaa, 0xaa, 0xaa, 0xaa};
    observed_len = sizeof(truncated);
    if (getpeername(client, (struct sockaddr *)truncated, &observed_len) < 0)
        fail("getpeername(truncated AF_UNIX)");
    if (observed_len != server_len || memcmp(truncated, &server_addr, sizeof(truncated)) != 0) {
        fprintf(stderr, "FAIL: truncated address len=%u expected=%u\n",
                (unsigned)observed_len, (unsigned)server_len);
        exit(1);
    }

    close(accepted);
    close(client);
    close(listener);
    if (!abstract) {
        unlink(server_path);
        unlink(client_path);
    }
}

int main(void)
{
    test_descriptor_and_connection_errors();
    test_output_validation_precedes_peer_lookup();
    test_named_unix_addresses(0);
    test_named_unix_addresses(1);
    puts("getpeername Linux probe: PASS");
    return 0;
}
