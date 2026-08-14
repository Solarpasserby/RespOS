#define _GNU_SOURCE

#include <errno.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static void fail(const char *what)
{
    perror(what);
    exit(1);
}

static struct ucred read_peercred(int fd)
{
    struct ucred cred = {0};
    socklen_t len = sizeof(cred);

    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &cred, &len) < 0)
        fail("getsockopt(SO_PEERCRED)");
    if (len != sizeof(cred)) {
        fprintf(stderr, "FAIL: SO_PEERCRED len=%u expected=%zu\n",
                (unsigned)len, sizeof(cred));
        exit(1);
    }
    return cred;
}

static void expect_cred(struct ucred cred, pid_t pid, uid_t uid, gid_t gid,
                        const char *what)
{
    if (cred.pid != pid || cred.uid != uid || cred.gid != gid) {
        fprintf(stderr,
                "FAIL: %s pid=%d uid=%u gid=%u expected=%d/%u/%u\n",
                what, cred.pid, cred.uid, cred.gid, pid, uid, gid);
        exit(1);
    }
}

static void test_socketpair_credentials(void)
{
    int sv[2];

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0)
        fail("socketpair");
    expect_cred(read_peercred(sv[0]), getpid(), getuid(), getgid(),
                "socketpair left");
    expect_cred(read_peercred(sv[1]), getpid(), getuid(), getgid(),
                "socketpair right");
    close(sv[0]);
    close(sv[1]);
}

static void test_accepted_peer_snapshot(void)
{
    char path[sizeof(((struct sockaddr_un *)0)->sun_path)];
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    int release_pipe[2];
    int listener;
    int accepted;
    pid_t child;
    pid_t listener_pid = getpid();
    uid_t listener_uid = getuid();
    gid_t listener_gid = getgid();
    int status;

    snprintf(path, sizeof(path), "/tmp/respos-peercred-%ld.sock",
             (long)getpid());
    if (strlen(path) >= sizeof(addr.sun_path)) {
        fprintf(stderr, "FAIL: unix socket path is too long\n");
        exit(1);
    }
    strcpy(addr.sun_path, path);
    unlink(path);

    listener = socket(AF_UNIX, SOCK_STREAM, 0);
    if (listener < 0)
        fail("socket(listener)");
    if (bind(listener, (struct sockaddr *)&addr, sizeof(addr)) < 0)
        fail("bind(listener)");
    if (listen(listener, 4) < 0)
        fail("listen");
    if (pipe(release_pipe) < 0)
        fail("pipe");

    child = fork();
    if (child < 0)
        fail("fork");
    if (child == 0) {
        char byte;
        int client;

        close(release_pipe[1]);
        client = socket(AF_UNIX, SOCK_STREAM, 0);
        if (client < 0)
            _exit(2);
        if (connect(client, (struct sockaddr *)&addr, sizeof(addr)) < 0)
            _exit(3);
        expect_cred(read_peercred(client), listener_pid, listener_uid,
                    listener_gid, "connecting peer");
        if (read(release_pipe[0], &byte, 1) != 1)
            _exit(4);
        close(client);
        _exit(0);
    }

    close(release_pipe[0]);
    accepted = accept(listener, NULL, NULL);
    if (accepted < 0)
        fail("accept");
    expect_cred(read_peercred(accepted), child, getuid(), getgid(),
                "accepted peer");
    if (write(release_pipe[1], "x", 1) != 1)
        fail("release child");
    close(release_pipe[1]);
    close(accepted);
    close(listener);
    if (waitpid(child, &status, 0) != child)
        fail("waitpid");
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        fprintf(stderr, "FAIL: child status=%d\n", status);
        exit(1);
    }
    unlink(path);
}

int main(void)
{
    test_socketpair_credentials();
    test_accepted_peer_snapshot();
    puts("socket peercred Linux probe: PASS");
    return 0;
}
