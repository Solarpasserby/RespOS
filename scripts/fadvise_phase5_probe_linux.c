#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

enum { PAGE_BYTES = 4096, FILE_PAGES = 4 };

static int resident(int fd, off_t offset)
{
    unsigned char vec = 0;
    void *mapping = mmap(NULL, PAGE_BYTES, PROT_READ, MAP_SHARED, fd, offset);
    assert(mapping != MAP_FAILED);
    assert(mincore(mapping, PAGE_BYTES, &vec) == 0);
    assert(munmap(mapping, PAGE_BYTES) == 0);
    return vec & 1;
}

static void fault_page(int fd, off_t offset, unsigned char expected)
{
    unsigned char *mapping = mmap(NULL, PAGE_BYTES, PROT_READ, MAP_SHARED, fd, offset);
    assert(mapping != MAP_FAILED);
    assert(mapping[0] == expected);
    assert(munmap(mapping, PAGE_BYTES) == 0);
}

int main(void)
{
    char path[] = "/tmp/respos-fadvise-XXXXXX";
    int fd = mkstemp(path);
    assert(fd >= 0);
    unsigned char page[PAGE_BYTES];
    for (int index = 0; index < FILE_PAGES; ++index) {
        memset(page, index + 1, sizeof(page));
        assert(write(fd, page, sizeof(page)) == (ssize_t)sizeof(page));
    }
    assert(fsync(fd) == 0);

    const int advice[] = {
        POSIX_FADV_NORMAL, POSIX_FADV_RANDOM, POSIX_FADV_SEQUENTIAL,
        POSIX_FADV_WILLNEED, POSIX_FADV_DONTNEED, POSIX_FADV_NOREUSE,
    };
    for (size_t index = 0; index < sizeof(advice) / sizeof(advice[0]); ++index)
        assert(posix_fadvise(fd, 0, 0, advice[index]) == 0);
    assert(posix_fadvise(fd, -1, 0, POSIX_FADV_NORMAL) == EINVAL);
    assert(posix_fadvise(fd, 0, -1, POSIX_FADV_NORMAL) == EINVAL);
    assert(posix_fadvise(fd, 0, 0, 99) == EINVAL);
    assert(posix_fadvise(-1, 0, 0, POSIX_FADV_NORMAL) == EBADF);
    int pipefd[2];
    assert(pipe(pipefd) == 0);
    assert(posix_fadvise(pipefd[0], 0, 0, POSIX_FADV_NORMAL) == ESPIPE);
    assert(close(pipefd[0]) == 0);
    assert(close(pipefd[1]) == 0);

    assert(posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED) == 0);
    fault_page(fd, 0, 1);
    assert(resident(fd, 0));
    assert(posix_fadvise(fd, 1, PAGE_BYTES - 2, POSIX_FADV_DONTNEED) == 0);
    assert(resident(fd, 0));
    assert(posix_fadvise(fd, 0, PAGE_BYTES, POSIX_FADV_DONTNEED) == 0);
    assert(!resident(fd, 0));

    assert(posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED) == 0);
    assert(posix_fadvise(fd, 2 * PAGE_BYTES, 1, POSIX_FADV_WILLNEED) == 0);
    /* WILLNEED initiates best-effort readahead; completion is asynchronous. */
    fault_page(fd, 2 * PAGE_BYTES, 3);

    memset(page, 0xa5, sizeof(page));
    assert(pwrite(fd, page, sizeof(page), 3 * PAGE_BYTES) == (ssize_t)sizeof(page));
    assert(posix_fadvise(fd, 3 * PAGE_BYTES, PAGE_BYTES, POSIX_FADV_DONTNEED) == 0);
    assert(fsync(fd) == 0);
    assert(posix_fadvise(fd, 3 * PAGE_BYTES, PAGE_BYTES, POSIX_FADV_DONTNEED) == 0);
    unsigned char value = 0;
    assert(pread(fd, &value, 1, 3 * PAGE_BYTES) == 1 && value == 0xa5);

    assert(close(fd) == 0);
    assert(unlink(path) == 0);
    puts("FADVISE_PHASE5_LINUX ALL PASS");
    return 0;
}
