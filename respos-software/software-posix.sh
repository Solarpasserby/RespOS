#!/bin/sh

set -u

WORK=/tmp/respos-software-posix
failures=0

export HOME=/tmp/respos-software-posix-home
export TMPDIR=/tmp
export LC_ALL=C
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

pass() {
    echo "SOFTWARE_POSIX $1 PASS"
}

fail() {
    echo "SOFTWARE_POSIX $1 FAIL"
    failures=$((failures + 1))
}

rm -rf "${WORK}" "${HOME}"
mkdir -p "${WORK}" "${HOME}"

echo "SOFTWARE_POSIX BEGIN"
uname -a || true

pipeline_ok=1
PIPE_WORK="${WORK}/pipeline"
mkdir -p "${PIPE_WORK}/input" || pipeline_ok=0
i=0
while [ "${i}" -lt 48 ]; do
    value=$((i % 12))
    printf '%02d payload-%02d\n' "${value}" "${i}" > "${PIPE_WORK}/input/item-${i}" || pipeline_ok=0
    i=$((i + 1))
done
find "${PIPE_WORK}/input" -type f -print0 \
    | xargs -0 -n 1 -P 2 sha256sum \
    | sort > "${PIPE_WORK}/parallel.sha" || pipeline_ok=0
find "${PIPE_WORK}/input" -type f -print0 \
    | xargs -0 -n 1 sha256sum \
    | sort > "${PIPE_WORK}/serial.sha" || pipeline_ok=0
cmp -s "${PIPE_WORK}/parallel.sha" "${PIPE_WORK}/serial.sha" || pipeline_ok=0
test "$(wc -l < "${PIPE_WORK}/parallel.sha")" -eq 48 || pipeline_ok=0

mkfifo "${PIPE_WORK}/events" || pipeline_ok=0
(while IFS= read -r line; do printf '%s\n' "${line}"; done \
    < "${PIPE_WORK}/events" > "${PIPE_WORK}/events.out") &
consumer_pid=$!
(
    i=0
    while [ "${i}" -lt 16 ]; do
        printf 'left-%02d\n' "${i}"
        i=$((i + 1))
    done
) > "${PIPE_WORK}/events" &
left_pid=$!
(
    i=0
    while [ "${i}" -lt 16 ]; do
        printf 'right-%02d\n' "${i}"
        i=$((i + 1))
    done
) > "${PIPE_WORK}/events" &
right_pid=$!
wait "${left_pid}" || pipeline_ok=0
wait "${right_pid}" || pipeline_ok=0
wait "${consumer_pid}" || pipeline_ok=0
sort -u "${PIPE_WORK}/events.out" > "${PIPE_WORK}/events.sorted" || pipeline_ok=0
test "$(wc -l < "${PIPE_WORK}/events.sorted")" -eq 32 || pipeline_ok=0
grep -qx left-00 "${PIPE_WORK}/events.sorted" || pipeline_ok=0
grep -qx right-15 "${PIPE_WORK}/events.sorted" || pipeline_ok=0
if [ "${pipeline_ok}" -eq 1 ]; then pass pipeline_fifo_parallel; else fail pipeline_fifo_parallel; fi

archive_ok=1
ARCHIVE_WORK="${WORK}/archive"
mkdir -p "${ARCHIVE_WORK}/source/tree" "${ARCHIVE_WORK}/extract" || archive_ok=0
printf 'archive payload\n' > "${ARCHIVE_WORK}/source/tree/original.txt" || archive_ok=0
ln "${ARCHIVE_WORK}/source/tree/original.txt" "${ARCHIVE_WORK}/source/tree/hard.txt" || archive_ok=0
ln -s original.txt "${ARCHIVE_WORK}/source/tree/symbolic.txt" || archive_ok=0
dd if=/dev/zero bs=4096 count=8 2>/dev/null \
    | tr '\000' 'Z' > "${ARCHIVE_WORK}/source/tree/block.bin" || archive_ok=0
(
    cd "${ARCHIVE_WORK}/source" || exit 1
    tar -cf - tree
) | gzip -c > "${ARCHIVE_WORK}/tree.tar.gz" || archive_ok=0
gzip -dc "${ARCHIVE_WORK}/tree.tar.gz" \
    | tar -xf - -C "${ARCHIVE_WORK}/extract" || archive_ok=0
cmp -s "${ARCHIVE_WORK}/source/tree/original.txt" \
    "${ARCHIVE_WORK}/extract/tree/original.txt" || archive_ok=0
cmp -s "${ARCHIVE_WORK}/source/tree/block.bin" \
    "${ARCHIVE_WORK}/extract/tree/block.bin" || archive_ok=0
test "$(readlink "${ARCHIVE_WORK}/extract/tree/symbolic.txt")" = original.txt || archive_ok=0
source_inode=$(stat -c %i "${ARCHIVE_WORK}/extract/tree/original.txt") || archive_ok=0
hard_inode=$(stat -c %i "${ARCHIVE_WORK}/extract/tree/hard.txt") || archive_ok=0
test "${source_inode}" = "${hard_inode}" || archive_ok=0
test "$(stat -c %h "${ARCHIVE_WORK}/extract/tree/original.txt")" -eq 2 || archive_ok=0
if [ "${archive_ok}" -eq 1 ]; then pass archive_link_pipeline; else fail archive_link_pipeline; fi

pipe_atomic_ok=1
PIPE_C_WORK="${WORK}/pipe-atomic"
mkdir -p "${PIPE_C_WORK}" || pipe_atomic_ok=0
cat > "${PIPE_C_WORK}/pipe-atomic.c" <<'EOF'
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

int main(void) {
    int fds[2];
    char page[4096];
    char drain[128];
    memset(page, 'P', sizeof(page));
    if (pipe2(fds, O_NONBLOCK) != 0) return 1;
    while (write(fds[1], page, sizeof(page)) == (ssize_t)sizeof(page)) {}
    if (errno != EAGAIN && errno != EWOULDBLOCK) return 2;
    if (read(fds[0], drain, sizeof(drain)) != (ssize_t)sizeof(drain)) return 3;

    struct iovec fits[2] = {
        { .iov_base = page, .iov_len = 32 },
        { .iov_base = page + 32, .iov_len = 32 },
    };
    if (writev(fds[1], fits, 2) != 64) return 4;

    struct iovec too_large[2] = {
        { .iov_base = page, .iov_len = 64 },
        { .iov_base = page + 64, .iov_len = 64 },
    };
    errno = 0;
    if (writev(fds[1], too_large, 2) != -1) return 5;
    if (errno != EAGAIN && errno != EWOULDBLOCK) return 6;
    puts("PIPE_ATOMIC_OK");
    return 0;
}
EOF
(
    cd "${PIPE_C_WORK}" || exit 1
    gcc -O2 -Wall -Wextra -Werror pipe-atomic.c -o pipe-atomic \
        > "${WORK}/pipe-atomic-compile.txt" 2>&1 || exit 1
    ./pipe-atomic > "${WORK}/pipe-atomic-output.txt" 2>&1 || exit 1
) || pipe_atomic_ok=0
grep -qx PIPE_ATOMIC_OK "${WORK}/pipe-atomic-output.txt" || pipe_atomic_ok=0
if [ "${pipe_atomic_ok}" -eq 1 ]; then pass pipe_nonblock_atomic; else fail pipe_nonblock_atomic; fi

flock_ok=1
FLOCK_WORK="${WORK}/flock"
mkdir -p "${FLOCK_WORK}" || flock_ok=0
rm -f "${FLOCK_WORK}/ready" "${FLOCK_WORK}/lock"
(
    exec 9> "${FLOCK_WORK}/lock" || exit 1
    flock -x 9 || exit 1
    printf 'ready\n' > "${FLOCK_WORK}/ready" || exit 1
    sleep 1
) &
holder_pid=$!
i=0
while [ "${i}" -lt 100 ] && [ ! -s "${FLOCK_WORK}/ready" ]; do
    sleep 0.02
    i=$((i + 1))
done
test -s "${FLOCK_WORK}/ready" || flock_ok=0
exec 8> "${FLOCK_WORK}/lock" || flock_ok=0
if flock -n -x 8; then
    flock_ok=0
    flock -u 8 || true
fi
wait "${holder_pid}" || flock_ok=0
flock -n -x 8 || flock_ok=0
flock -u 8 || flock_ok=0
if [ "${flock_ok}" -eq 1 ]; then pass flock_contention_release; else fail flock_contention_release; fi

lock_lifecycle_ok=1
LOCK_C_WORK="${WORK}/lock-lifecycle"
mkdir -p "${LOCK_C_WORK}" || lock_lifecycle_ok=0
cat > "${LOCK_C_WORK}/lock-lifecycle.c" <<'EOF'
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/file.h>
#include <sys/wait.h>
#include <unistd.h>

static int wait_ok(pid_t pid, int expected) {
    int status = 0;
    return waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == expected;
}

static int child_flock(const char *path, int expect_busy) {
    pid_t pid = fork();
    if (pid == 0) {
        int fd = open(path, O_RDWR | O_CREAT, 0600);
        if (fd < 0) _exit(20);
        int rc = flock(fd, LOCK_EX | LOCK_NB);
        if (expect_busy) _exit(rc == -1 && (errno == EWOULDBLOCK || errno == EAGAIN) ? 0 : 21);
        _exit(rc == 0 ? 0 : 22);
    }
    return pid > 0 && wait_ok(pid, 0);
}

static int set_record_lock(int fd) {
    struct flock lock = {
        .l_type = F_WRLCK,
        .l_whence = SEEK_SET,
        .l_start = 0,
        .l_len = 0,
    };
    return fcntl(fd, F_SETLK, &lock);
}

static int child_record_lock(const char *path) {
    pid_t pid = fork();
    if (pid == 0) {
        int fd = open(path, O_RDWR | O_CREAT, 0600);
        if (fd < 0) _exit(30);
        _exit(set_record_lock(fd) == 0 ? 0 : 31);
    }
    return pid > 0 && wait_ok(pid, 0);
}

int main(void) {
    int fd = open("flock-file", O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || flock(fd, LOCK_EX) != 0) return 1;
    int duplicate = dup(fd);
    if (duplicate < 0 || close(fd) != 0) return 2;
    if (!child_flock("flock-file", 1)) return 3;
    if (close(duplicate) != 0) return 4;
    if (!child_flock("flock-file", 0)) return 5;

    fd = open("record-close-file", O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || set_record_lock(fd) != 0) return 6;
    duplicate = dup(fd);
    if (duplicate < 0 || close(duplicate) != 0) return 7;
    if (!child_record_lock("record-close-file")) return 8;
    close(fd);

    pid_t owner = fork();
    if (owner == 0) {
        int owned = open("record-exit-file", O_RDWR | O_CREAT | O_TRUNC, 0600);
        _exit(owned >= 0 && set_record_lock(owned) == 0 ? 0 : 40);
    }
    if (owner < 0 || !wait_ok(owner, 0)) return 9;
    if (!child_record_lock("record-exit-file")) return 10;

    puts("LOCK_LIFECYCLE_OK");
    return 0;
}
EOF
(
    cd "${LOCK_C_WORK}" || exit 1
    gcc -O2 -Wall -Wextra -Werror lock-lifecycle.c -o lock-lifecycle \
        > "${WORK}/lock-compile.txt" 2>&1 || exit 1
    ./lock-lifecycle > "${WORK}/lock-output.txt" 2>&1 || exit 1
) || lock_lifecycle_ok=0
grep -qx LOCK_LIFECYCLE_OK "${WORK}/lock-output.txt" || lock_lifecycle_ok=0
if [ "${lock_lifecycle_ok}" -eq 1 ]; then pass lock_lifecycle; else fail lock_lifecycle; fi

apk_ok=1
/sbin/apk info -e busybox || apk_ok=0
/sbin/apk info -e musl || apk_ok=0
/sbin/apk info -W /bin/sh > "${WORK}/apk-owner.txt" || apk_ok=0
grep -q busybox "${WORK}/apk-owner.txt" || apk_ok=0
/sbin/apk info > "${WORK}/apk-installed.txt" || apk_ok=0
test "$(wc -l < "${WORK}/apk-installed.txt")" -gt 10 || apk_ok=0
if [ "${apk_ok}" -eq 1 ]; then pass apk_local_database; else fail apk_local_database; fi

if [ "${failures}" -eq 0 ]; then
    echo "SOFTWARE_POSIX ALL PASS"
    exit 0
fi

echo "SOFTWARE_POSIX ALL FAIL failures=${failures}"
exit 1
