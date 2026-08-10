#define _GNU_SOURCE

#include <assert.h>
#include <fcntl.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define DIR_A "/dev/shm/respos-ns-a"
#define DIR_B "/dev/shm/respos-ns-b"
#define SOURCE DIR_A "/source"
#define ALIAS DIR_A "/alias"
#define MOVED DIR_B "/moved"
#define REPLACE_SRC DIR_A "/replace-src"
#define REPLACE_DST DIR_A "/replace-dst"
#define SUB_OLD DIR_A "/sub"
#define SUB_NEW DIR_B "/sub"
#define CHILD_OLD SUB_OLD "/child"
#define CHILD_NEW SUB_NEW "/child"
#define RACE_A DIR_A "/race-a"
#define RACE_B DIR_B "/race-b"
#define DIR_REPLACE_SRC DIR_A "/dir-replace-src"
#define DIR_REPLACE_DST DIR_A "/dir-replace-dst"

static struct stat path_stat(const char *path) {
    struct stat value;
    assert(stat(path, &value) == 0);
    return value;
}

static struct stat fd_stat(int fd) {
    struct stat value;
    assert(fstat(fd, &value) == 0);
    return value;
}

static int create_file(const char *path, const char *data) {
    int fd = open(path, O_CREAT | O_TRUNC | O_RDWR, 0640);
    assert(fd >= 0);
    assert(write(fd, data, strlen(data)) == (ssize_t)strlen(data));
    return fd;
}

static void cleanup(void) {
    const char *files[] = {CHILD_OLD, CHILD_NEW, SOURCE, ALIAS, MOVED,
                           REPLACE_SRC, REPLACE_DST, RACE_A, RACE_B};
    for (size_t i = 0; i < sizeof(files) / sizeof(files[0]); ++i)
        (void)unlink(files[i]);
    (void)rmdir(SUB_OLD);
    (void)rmdir(SUB_NEW);
    (void)rmdir(DIR_REPLACE_SRC);
    (void)rmdir(DIR_REPLACE_DST);
    (void)rmdir(DIR_A);
    (void)rmdir(DIR_B);
}

int main(void) {
    cleanup();
    assert(mkdir(DIR_A, 0755) == 0);
    assert(mkdir(DIR_B, 0755) == 0);

    int source_fd = create_file(SOURCE, "source");
    ino_t source_ino = fd_stat(source_fd).st_ino;
    assert(close(source_fd) == 0);
    source_fd = open(SOURCE, O_RDONLY);
    assert(source_fd >= 0 && fd_stat(source_fd).st_ino == source_ino);
    assert(close(source_fd) == 0);

    assert(link(SOURCE, ALIAS) == 0);
    assert(path_stat(SOURCE).st_ino == source_ino);
    assert(path_stat(ALIAS).st_ino == source_ino);
    assert(path_stat(ALIAS).st_nlink == 2);
    assert(rename(SOURCE, MOVED) == 0);
    assert(access(SOURCE, F_OK) == -1);
    assert(path_stat(MOVED).st_ino == source_ino);
    assert(path_stat(ALIAS).st_ino == source_ino);

    int moved_fd = open(MOVED, O_RDWR);
    assert(moved_fd >= 0);
    assert(unlink(MOVED) == 0);
    assert(fd_stat(moved_fd).st_nlink == 1);
    assert(unlink(ALIAS) == 0);
    assert(fd_stat(moved_fd).st_nlink == 0);
    assert(lseek(moved_fd, 0, SEEK_SET) == 0);
    char source_data[6];
    assert(read(moved_fd, source_data, sizeof(source_data)) == 6);
    assert(memcmp(source_data, "source", 6) == 0);
    assert(close(moved_fd) == 0);

    int replace_src = create_file(REPLACE_SRC, "new");
    ino_t replace_src_ino = fd_stat(replace_src).st_ino;
    assert(close(replace_src) == 0);
    int replace_dst = create_file(REPLACE_DST, "old");
    ino_t replace_dst_ino = fd_stat(replace_dst).st_ino;
    assert(replace_src_ino != replace_dst_ino);
    assert(rename(REPLACE_SRC, REPLACE_DST) == 0);
    assert(path_stat(REPLACE_DST).st_ino == replace_src_ino);
    assert(fd_stat(replace_dst).st_ino == replace_dst_ino);
    assert(fd_stat(replace_dst).st_nlink == 0);
    assert(lseek(replace_dst, 0, SEEK_SET) == 0);
    char old_data[3];
    assert(read(replace_dst, old_data, sizeof(old_data)) == 3);
    assert(memcmp(old_data, "old", 3) == 0);
    assert(close(replace_dst) == 0);

    assert(mkdir(DIR_REPLACE_SRC, 0755) == 0);
    assert(mkdir(DIR_REPLACE_DST, 0755) == 0);
    int replaced_dir = open(DIR_REPLACE_DST, O_RDONLY | O_DIRECTORY);
    assert(replaced_dir >= 0);
    ino_t replaced_dir_ino = fd_stat(replaced_dir).st_ino;
    assert(rename(DIR_REPLACE_SRC, DIR_REPLACE_DST) == 0);
    assert(path_stat(DIR_REPLACE_DST).st_ino != replaced_dir_ino);
    assert(fd_stat(replaced_dir).st_ino == replaced_dir_ino);
    assert(fd_stat(replaced_dir).st_nlink == 0);
    assert(close(replaced_dir) == 0);

    nlink_t a_before = path_stat(DIR_A).st_nlink;
    nlink_t b_before = path_stat(DIR_B).st_nlink;
    assert(mkdir(SUB_OLD, 0755) == 0);
    int child = create_file(CHILD_OLD, "child");
    ino_t child_ino = fd_stat(child).st_ino;
    assert(path_stat(DIR_A).st_nlink == a_before + 1);
    assert(rename(SUB_OLD, SUB_NEW) == 0);
    assert(path_stat(DIR_A).st_nlink == a_before);
    assert(path_stat(DIR_B).st_nlink == b_before + 1);
    assert(path_stat(CHILD_NEW).st_ino == child_ino);
    assert(fd_stat(child).st_ino == child_ino);
    assert(close(child) == 0);

    int race = create_file(RACE_A, "race");
    ino_t race_ino = fd_stat(race).st_ino;
    assert(close(race) == 0);
    pid_t child_pid = fork();
    assert(child_pid >= 0);
    if (child_pid == 0) {
        for (int i = 0; i < 200; ++i) {
            assert(rename(RACE_A, RACE_B) == 0);
            assert(rename(RACE_B, RACE_A) == 0);
        }
        _exit(0);
    }
    size_t observations = 0;
    for (int i = 0; i < 1000; ++i) {
        const char *paths[] = {RACE_A, RACE_B};
        for (size_t j = 0; j < 2; ++j) {
            int fd = open(paths[j], O_RDONLY);
            if (fd >= 0) {
                assert(fd_stat(fd).st_ino == race_ino);
                assert(close(fd) == 0);
                ++observations;
            }
        }
        (void)sched_yield();
    }
    int status = 0;
    assert(waitpid(child_pid, &status, 0) == child_pid);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(observations > 0);

    cleanup();
    printf("FS_NAMESPACE_PROBE_PASS race_observations=%zu\n", observations);
    return 0;
}
