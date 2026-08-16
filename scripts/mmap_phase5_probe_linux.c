#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

enum { PAGE_SIZE_ = 4096, MAP_SIZE = 3 * PAGE_SIZE_ };

static void pipe_send(int fd)
{
    const unsigned char byte = 1;
    assert(write(fd, &byte, 1) == 1);
}

static void pipe_recv(int fd)
{
    unsigned char byte = 0;
    assert(read(fd, &byte, 1) == 1);
}

static void close_pipe(int pipefd[2])
{
    assert(close(pipefd[0]) == 0);
    assert(close(pipefd[1]) == 0);
}

static void expect_child_sigbus(pid_t child)
{
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFSIGNALED(status));
    assert(WTERMSIG(status) == SIGBUS);
}

static void expect_sigbus(volatile unsigned char *address)
{
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        volatile unsigned char value = *address;
        (void)value;
        _exit(99);
    }

    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFSIGNALED(status));
    assert(WTERMSIG(status) == SIGBUS);
}

static void write_byte(int fd, off_t offset, unsigned char value)
{
    assert(pwrite(fd, &value, 1, offset) == 1);
}

static void test_mode(int fd, int map_flag, const char *label)
{
    assert(ftruncate(fd, PAGE_SIZE_ + 128) == 0);
    write_byte(fd, PAGE_SIZE_ + 64, 0x5a);
    unsigned char *mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                  map_flag, fd, 0);
    assert(mapping != MAP_FAILED);
    assert(mapping[PAGE_SIZE_ + 64] == 0x5a);
    assert(mapping[PAGE_SIZE_ + 512] == 0);
    expect_sigbus(mapping + 2 * PAGE_SIZE_);
    assert(munmap(mapping, MAP_SIZE) == 0);

    assert(ftruncate(fd, MAP_SIZE) == 0);
    write_byte(fd, PAGE_SIZE_ + 512, 0x7c);
    write_byte(fd, 2 * PAGE_SIZE_ + 17, 0x6d);
    mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE, map_flag, fd, 0);
    assert(mapping != MAP_FAILED);
    assert(mapping[PAGE_SIZE_ + 512] == 0x7c);
    assert(mapping[2 * PAGE_SIZE_ + 17] == 0x6d);
    assert(ftruncate(fd, PAGE_SIZE_ + 128) == 0);
    assert(mapping[PAGE_SIZE_ + 512] == 0);
    expect_sigbus(mapping + 2 * PAGE_SIZE_ + 17);
    assert(munmap(mapping, MAP_SIZE) == 0);

    assert(ftruncate(fd, PAGE_SIZE_) == 0);
    mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE, map_flag, fd, 0);
    assert(mapping != MAP_FAILED);
    assert(ftruncate(fd, MAP_SIZE) == 0);
    write_byte(fd, 2 * PAGE_SIZE_ + 33, 0xa7);
    assert(mapping[2 * PAGE_SIZE_ + 33] == 0xa7);
    assert(munmap(mapping, MAP_SIZE) == 0);

    printf("MMAP_PHASE5_LINUX %s PASS\n", label);
}

static void test_private_cow_truncate(int fd)
{
    assert(ftruncate(fd, MAP_SIZE) == 0);
    unsigned char *mapping = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE, fd, 0);
    assert(mapping != MAP_FAILED);
    mapping[PAGE_SIZE_ + 64] = 0x44;
    mapping[PAGE_SIZE_ + 512] = 0x55;
    mapping[2 * PAGE_SIZE_ + 17] = 0x66;
    assert(ftruncate(fd, PAGE_SIZE_ + 128) == 0);
    assert(mapping[PAGE_SIZE_ + 64] == 0x44);
    /* A private COW page keeps its anonymous bytes even past partial EOF. */
    assert(mapping[PAGE_SIZE_ + 512] == 0x55);
    /* Whole pages beyond the new EOF are invalidated even after COW. */
    expect_sigbus(mapping + 2 * PAGE_SIZE_ + 17);
    assert(munmap(mapping, MAP_SIZE) == 0);
    puts("MMAP_PHASE5_LINUX private_cow_truncate PASS");
}

static pid_t spawn_truncate_mapper(int fd, int ready_fd, int go_fd,
                                   int prefault)
{
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        volatile unsigned char *mapping = mmap(NULL, MAP_SIZE,
                                               PROT_READ | PROT_WRITE,
                                               MAP_SHARED, fd, 0);
        assert(mapping != MAP_FAILED);
        if (prefault) {
            volatile unsigned char value = mapping[2 * PAGE_SIZE_ + 17];
            (void)value;
        }
        pipe_send(ready_fd);
        pipe_recv(go_fd);
        volatile unsigned char value = mapping[2 * PAGE_SIZE_ + 17];
        (void)value;
        _exit(99);
    }
    return child;
}

static void test_cross_process_truncate(int fd)
{
    int ready[2], resident_go[2], unfaulted_go[2], truncate_go[2];
    int truncate_done[2];
    assert(pipe(ready) == 0);
    assert(pipe(resident_go) == 0);
    assert(pipe(unfaulted_go) == 0);
    assert(pipe(truncate_go) == 0);
    assert(pipe(truncate_done) == 0);
    assert(ftruncate(fd, MAP_SIZE) == 0);
    write_byte(fd, 2 * PAGE_SIZE_ + 17, 0x81);

    pid_t resident = spawn_truncate_mapper(fd, ready[1], resident_go[0], 1);
    pid_t unfaulted = spawn_truncate_mapper(fd, ready[1], unfaulted_go[0], 0);
    pid_t truncator = fork();
    assert(truncator >= 0);
    if (truncator == 0) {
        pipe_recv(truncate_go[0]);
        assert(ftruncate(fd, PAGE_SIZE_) == 0);
        pipe_send(truncate_done[1]);
        _exit(0);
    }

    pipe_recv(ready[0]);
    pipe_recv(ready[0]);
    pipe_send(truncate_go[1]);
    pipe_recv(truncate_done[0]);
    pipe_send(resident_go[1]);
    pipe_send(unfaulted_go[1]);
    expect_child_sigbus(resident);
    expect_child_sigbus(unfaulted);
    int status = -1;
    assert(waitpid(truncator, &status, 0) == truncator);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    close_pipe(ready);
    close_pipe(resident_go);
    close_pipe(unfaulted_go);
    close_pipe(truncate_go);
    close_pipe(truncate_done);
    puts("MMAP_PHASE5_LINUX cross_process_truncate PASS");
}

static void test_cross_process_fault_store_race(int fd)
{
    enum { ROUNDS = 16 };
    for (int round = 0; round < ROUNDS; ++round) {
        int ready[2], mapper_go[2], truncate_go[2], truncate_done[2];
        assert(pipe(ready) == 0);
        assert(pipe(mapper_go) == 0);
        assert(pipe(truncate_go) == 0);
        assert(pipe(truncate_done) == 0);
        assert(ftruncate(fd, MAP_SIZE) == 0);

        pid_t mapper = fork();
        assert(mapper >= 0);
        if (mapper == 0) {
            volatile unsigned char *mapping = mmap(NULL, MAP_SIZE,
                                                   PROT_READ | PROT_WRITE,
                                                   MAP_SHARED, fd, 0);
            assert(mapping != MAP_FAILED);
            pipe_send(ready[1]);
            pipe_recv(mapper_go[0]);
            mapping[2 * PAGE_SIZE_ + 31] = (unsigned char)round;
            pipe_recv(truncate_done[0]);
            mapping[2 * PAGE_SIZE_ + 31] ^= 1;
            _exit(99);
        }

        pid_t truncator = fork();
        assert(truncator >= 0);
        if (truncator == 0) {
            pipe_recv(truncate_go[0]);
            assert(ftruncate(fd, PAGE_SIZE_) == 0);
            pipe_send(truncate_done[1]);
            _exit(0);
        }

        pipe_recv(ready[0]);
        pipe_send(mapper_go[1]);
        pipe_send(truncate_go[1]);
        expect_child_sigbus(mapper);
        int status = -1;
        assert(waitpid(truncator, &status, 0) == truncator);
        assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
        close_pipe(ready);
        close_pipe(mapper_go);
        close_pipe(truncate_go);
        close_pipe(truncate_done);
    }
    puts("MMAP_PHASE5_LINUX cross_process_fault_store_race PASS");
}

static void expect_zero_range(int fd, off_t offset, size_t length)
{
    unsigned char buffer[PAGE_SIZE_];
    while (length != 0) {
        size_t count = length < sizeof(buffer) ? length : sizeof(buffer);
        assert(pread(fd, buffer, count, offset) == (ssize_t)count);
        for (size_t index = 0; index < count; ++index)
            assert(buffer[index] == 0);
        offset += (off_t)count;
        length -= count;
    }
}

static void test_punch_hole(int fd)
{
    const off_t punch_start = PAGE_SIZE_ / 2;
    const off_t punch_length = 2 * PAGE_SIZE_;
    const off_t punch_end = punch_start + punch_length;
    unsigned char page[PAGE_SIZE_];
    for (size_t index = 0; index < sizeof(page); ++index)
        page[index] = (unsigned char)(index % 251 + 1);
    assert(ftruncate(fd, MAP_SIZE) == 0);
    for (off_t offset = 0; offset < MAP_SIZE; offset += PAGE_SIZE_)
        assert(pwrite(fd, page, sizeof(page), offset) == (ssize_t)sizeof(page));

    unsigned char *shared = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                 MAP_SHARED, fd, 0);
    unsigned char *clean = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                MAP_PRIVATE, fd, 0);
    unsigned char *cow = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                              MAP_PRIVATE, fd, 0);
    assert(shared != MAP_FAILED && clean != MAP_FAILED && cow != MAP_FAILED);
    volatile unsigned char prefault = shared[punch_start + 17] ^
                                      shared[PAGE_SIZE_ + 17] ^
                                      clean[punch_start + 17] ^
                                      clean[PAGE_SIZE_ + 17] ^
                                      cow[punch_start + 17] ^
                                      cow[PAGE_SIZE_ + 17];
    (void)prefault;
    cow[PAGE_SIZE_ + 17] = 0xa5;
    cow[2 * PAGE_SIZE_ + 17] = 0xb6;

    struct stat before, after;
    assert(fstat(fd, &before) == 0);
    assert(lseek(fd, 123, SEEK_SET) == 123);
    assert(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
                     punch_start, punch_length) == 0);
    assert(lseek(fd, 0, SEEK_CUR) == 123);
    assert(fstat(fd, &after) == 0);
    assert(after.st_size == MAP_SIZE);
    assert(after.st_blocks < before.st_blocks);
    expect_zero_range(fd, punch_start, punch_length);
    assert(shared[punch_start + 17] == 0);
    assert(shared[PAGE_SIZE_ + 17] == 0);
    assert(shared[2 * PAGE_SIZE_ + 17] == 0);
    assert(clean[punch_start + 17] == 0);
    assert(clean[PAGE_SIZE_ + 17] == 0);
    assert(clean[2 * PAGE_SIZE_ + 17] == 0);
    assert(cow[PAGE_SIZE_ + 17] == 0xa5);
    assert(cow[2 * PAGE_SIZE_ + 17] == 0xb6);
    assert(shared[punch_start - 1] == page[(punch_start - 1) % PAGE_SIZE_]);
    assert(shared[punch_end] == page[punch_end % PAGE_SIZE_]);
    assert(munmap(shared, MAP_SIZE) == 0);
    assert(munmap(clean, MAP_SIZE) == 0);
    assert(munmap(cow, MAP_SIZE) == 0);
    assert(fsync(fd) == 0);
    assert(posix_fadvise(fd, 0, MAP_SIZE, POSIX_FADV_DONTNEED) == 0);
    expect_zero_range(fd, punch_start, punch_length);
    puts("MMAP_PHASE5_LINUX punch_hole PASS");
}

static void test_cross_process_punch_hole(int fd)
{
    unsigned char page[PAGE_SIZE_];
    memset(page, 0x6d, sizeof(page));
    assert(ftruncate(fd, MAP_SIZE) == 0);
    for (off_t offset = 0; offset < MAP_SIZE; offset += PAGE_SIZE_)
        assert(pwrite(fd, page, sizeof(page), offset) == (ssize_t)sizeof(page));
    assert(fsync(fd) == 0);

    int ready[2], go[2];
    assert(pipe(ready) == 0);
    assert(pipe(go) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        unsigned char *shared = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                     MAP_SHARED, fd, 0);
        unsigned char *clean = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                    MAP_PRIVATE, fd, 0);
        unsigned char *cow = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE, fd, 0);
        assert(shared != MAP_FAILED && clean != MAP_FAILED && cow != MAP_FAILED);
        volatile unsigned char prefault = shared[PAGE_SIZE_ + 31] ^
                                          clean[PAGE_SIZE_ + 31];
        (void)prefault;
        cow[PAGE_SIZE_ + 31] = 0xa5;
        pipe_send(ready[1]);
        pipe_recv(go[0]);
        _exit(shared[PAGE_SIZE_ + 31] == 0 &&
              clean[PAGE_SIZE_ + 31] == 0 &&
              cow[PAGE_SIZE_ + 31] == 0xa5 ? 0 : 98);
    }

    pipe_recv(ready[0]);
    assert(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
                     PAGE_SIZE_, PAGE_SIZE_) == 0);
    pipe_send(go[1]);
    int status = -1;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    close_pipe(ready);
    close_pipe(go);
    puts("MMAP_PHASE5_LINUX cross_process_punch_hole PASS");
}

static void test_punch_hole_errors(int fd)
{
    struct stat before, after;
    assert(fstat(fd, &before) == 0);

    errno = 0;
    assert(fallocate(fd, FALLOC_FL_PUNCH_HOLE, 0, PAGE_SIZE_) == -1);
    assert(errno == EOPNOTSUPP);
    errno = 0;
    assert(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
                     0, 0) == -1);
    assert(errno == EINVAL);
    errno = 0;
    assert(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
                     -1, PAGE_SIZE_) == -1);
    assert(errno == EINVAL);

    assert(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
                     MAP_SIZE + PAGE_SIZE_, PAGE_SIZE_) == 0);
    assert(fstat(fd, &after) == 0);
    assert(after.st_size == before.st_size);
    assert(after.st_blocks == before.st_blocks);
    puts("MMAP_PHASE5_LINUX punch_hole_errors PASS");
}

static void test_punch_msync_race(int fd)
{
    enum { ROUNDS = 16 };
    unsigned char page[PAGE_SIZE_];
    memset(page, 0x6d, sizeof(page));
    for (int round = 0; round < ROUNDS; ++round) {
        assert(ftruncate(fd, PAGE_SIZE_) == 0);
        assert(pwrite(fd, page, sizeof(page), 0) == (ssize_t)sizeof(page));
        assert(fsync(fd) == 0);

        int ready[2], go[2];
        assert(pipe(ready) == 0);
        assert(pipe(go) == 0);
        pid_t mapper = fork();
        assert(mapper >= 0);
        if (mapper == 0) {
            volatile unsigned char *mapping = mmap(NULL, PAGE_SIZE_,
                                                    PROT_READ | PROT_WRITE,
                                                    MAP_SHARED, fd, 0);
            assert(mapping != MAP_FAILED);
            pipe_send(ready[1]);
            pipe_recv(go[0]);
            mapping[31] = (unsigned char)(round + 1);
            assert(msync((void *)mapping, PAGE_SIZE_, MS_SYNC) == 0);
            _exit(0);
        }

        pid_t puncher = fork();
        assert(puncher >= 0);
        if (puncher == 0) {
            pipe_recv(go[0]);
            assert(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
                             0, PAGE_SIZE_) == 0);
            _exit(0);
        }

        pipe_recv(ready[0]);
        pipe_send(go[1]);
        pipe_send(go[1]);
        int mapper_status = -1, puncher_status = -1;
        assert(waitpid(mapper, &mapper_status, 0) == mapper);
        assert(waitpid(puncher, &puncher_status, 0) == puncher);
        assert(WIFEXITED(mapper_status) && WEXITSTATUS(mapper_status) == 0);
        assert(WIFEXITED(puncher_status) && WEXITSTATUS(puncher_status) == 0);
        unsigned char byte = 0xff;
        assert(pread(fd, &byte, 1, 31) == 1);
        assert(byte == 0 || byte == (unsigned char)(round + 1));
        close_pipe(ready);
        close_pipe(go);
    }
    puts("MMAP_PHASE5_LINUX punch_msync_race PASS");
}

static void test_shared_write_enospc_sigbus(void)
{
    const char *directory = getenv("RESPOS_MMAP_ENOSPC_DIR");
    if (directory == NULL || directory[0] == '\0') {
        puts("MMAP_PHASE5_LINUX shared_write_enospc_sigbus SKIP (set RESPOS_MMAP_ENOSPC_DIR to a disposable small filesystem)");
        return;
    }

    char target_path[PATH_MAX], filler_path[PATH_MAX];
    assert(snprintf(target_path, sizeof(target_path), "%s/%s", directory,
                    "mmap-enospc-target") > 0);
    assert(snprintf(filler_path, sizeof(filler_path), "%s/%s", directory,
                    "mmap-enospc-filler") > 0);
    unlink(target_path);
    unlink(filler_path);
    int target = open(target_path, O_CREAT | O_TRUNC | O_RDWR, 0600);
    assert(target >= 0);
    assert(ftruncate(target, PAGE_SIZE_) == 0);
    assert(fsync(target) == 0);
    int filler = open(filler_path, O_CREAT | O_TRUNC | O_RDWR, 0600);
    assert(filler >= 0);

    unsigned char fill[16 * PAGE_SIZE_];
    memset(fill, 0x5a, sizeof(fill));
    int reached_enospc = 0;
    for (size_t chunk = 0; chunk < 4096; ++chunk) {
        ssize_t written = pwrite(filler, fill, sizeof(fill),
                                 (off_t)(chunk * sizeof(fill)));
        if (written < 0 && errno == ENOSPC) {
            reached_enospc = 1;
            break;
        }
        assert(written == (ssize_t)sizeof(fill));
        if (fsync(filler) < 0) {
            assert(errno == ENOSPC);
            reached_enospc = 1;
            break;
        }
    }
    assert(reached_enospc);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        volatile unsigned char *mapping = mmap(NULL, PAGE_SIZE_,
                                                PROT_READ | PROT_WRITE,
                                                MAP_SHARED, target, 0);
        assert(mapping != MAP_FAILED);
        mapping[0] = 0xa5;
        _exit(99);
    }
    expect_child_sigbus(child);

    assert(ftruncate(filler, 0) == 0);
    assert(close(filler) == 0);
    assert(unlink(filler_path) == 0);

    volatile unsigned char *recovered = mmap(NULL, PAGE_SIZE_,
                                              PROT_READ | PROT_WRITE,
                                              MAP_SHARED, target, 0);
    assert(recovered != MAP_FAILED);
    recovered[0] = 0x3c;
    assert(msync((void *)recovered, PAGE_SIZE_, MS_SYNC) == 0);
    assert(munmap((void *)recovered, PAGE_SIZE_) == 0);
    unsigned char byte = 0;
    assert(pread(target, &byte, 1, 0) == 1);
    assert(byte == 0x3c);
    assert(close(target) == 0);
    assert(unlink(target_path) == 0);
    puts("MMAP_PHASE5_LINUX shared_write_enospc_sigbus PASS");
}

int main(void)
{
    setbuf(stdout, NULL);
    char path[] = "/tmp/respos-mmap-phase5-XXXXXX";
    int fd = mkstemp(path);
    assert(fd >= 0);
    assert(unlink(path) == 0);

    test_mode(fd, MAP_SHARED, "shared");
    assert(ftruncate(fd, 0) == 0);
    test_mode(fd, MAP_PRIVATE, "private");
    test_private_cow_truncate(fd);
    test_cross_process_truncate(fd);
    test_cross_process_fault_store_race(fd);
    test_punch_hole(fd);
    test_cross_process_punch_hole(fd);
    test_punch_hole_errors(fd);
    test_punch_msync_race(fd);
    test_shared_write_enospc_sigbus();

    assert(close(fd) == 0);
    puts("MMAP_PHASE5_LINUX ALL PASS");
    return 0;
}
