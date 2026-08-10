#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#define FILE_PATH "/tmp/respos-fsmeta-file"
#define LINK_PATH "/tmp/respos-fsmeta-link"
#define UNLINKED_PATH "/tmp/respos-fsmeta-unlinked"
#define DIR_PATH "/tmp/respos-fsmeta-dir"

static void fail(const char *operation)
{
	fprintf(stderr, "FS_METADATA_LINUX_FAIL operation=%s errno=%d (%s)\n",
		operation, errno, strerror(errno));
	exit(1);
}

static void require(int condition, const char *operation)
{
	if (!condition)
		fail(operation);
}

static mode_t permission_bits(const struct stat *st)
{
	return st->st_mode & 07777;
}

static struct stat path_stat(const char *path)
{
	struct stat st;
	require(stat(path, &st) == 0, "stat");
	return st;
}

static struct stat fd_stat(int fd)
{
	struct stat st;
	require(fstat(fd, &st) == 0, "fstat");
	return st;
}

static void cleanup(void)
{
	unlink(LINK_PATH);
	unlink(FILE_PATH);
	unlink(UNLINKED_PATH);
	rmdir(DIR_PATH);
}

static void run_normal(void)
{
	struct stat original, alias, after;
	struct timespec times[2] = {
		{ .tv_sec = 1, .tv_nsec = 0 },
		{ .tv_sec = 2, .tv_nsec = 0 },
	};
	int fd, reopened, unlinked;

	cleanup();
	fd = open(FILE_PATH, O_CREAT | O_TRUNC | O_RDWR, 0640);
	require(fd >= 0, "create");
	require(write(fd, "metadata", 8) == 8, "write");
	require(link(FILE_PATH, LINK_PATH) == 0, "link");

	original = path_stat(FILE_PATH);
	alias = path_stat(LINK_PATH);
	require(original.st_ino == alias.st_ino, "hardlink inode identity");
	require(original.st_nlink == 2 && alias.st_nlink == 2, "hardlink nlink");

	require(chmod(FILE_PATH, 0601) == 0, "chmod");
	alias = path_stat(LINK_PATH);
	require(permission_bits(&alias) == 0601, "hardlink mode alias");

	require(chown(LINK_PATH, getuid(), getgid()) == 0, "chown");
	original = path_stat(FILE_PATH);
	alias = path_stat(LINK_PATH);
	require(original.st_uid == getuid() && original.st_gid == getgid(), "source owner");
	require(alias.st_uid == getuid() && alias.st_gid == getgid(), "alias owner");

	require(utimensat(AT_FDCWD, FILE_PATH, times, 0) == 0, "utimensat");
	alias = path_stat(LINK_PATH);
	require(alias.st_atim.tv_sec == 1 && alias.st_mtim.tv_sec == 2,
		"hardlink time alias");

	require(fsync(fd) == 0, "fsync");
	require(close(fd) == 0, "close");
	reopened = open(LINK_PATH, O_RDONLY);
	require(reopened >= 0, "reopen alias");
	after = fd_stat(reopened);
	require(after.st_ino == alias.st_ino, "reopen inode");
	require(permission_bits(&after) == 0601, "reopen mode");
	require(after.st_uid == getuid() && after.st_gid == getgid(), "reopen owner");
	require(close(reopened) == 0, "close reopened");

	unlinked = open(UNLINKED_PATH, O_CREAT | O_TRUNC | O_RDWR, 0640);
	require(unlinked >= 0, "create unlinked");
	require(unlink(UNLINKED_PATH) == 0, "unlink open file");
	require(fchmod(unlinked, 0600) == 0, "unlinked fchmod");
	require(fchown(unlinked, getuid(), getgid()) == 0, "unlinked fchown");
	times[0].tv_sec = 3;
	times[1].tv_sec = 4;
	require(futimens(unlinked, times) == 0, "unlinked futimens");
	after = fd_stat(unlinked);
	require(permission_bits(&after) == 0600, "unlinked fchmod mode");
	require(after.st_uid == getuid() && after.st_gid == getgid(),
		"unlinked fchown owner");
	require(after.st_atim.tv_sec == 3 && after.st_mtim.tv_sec == 4,
		"unlinked futimens times");
	require(close(unlinked) == 0, "close unlinked");
	puts("FS_METADATA_UNLINKED_FD_ATTRIBUTES_PASS");

	cleanup();
	puts("FS_METADATA_PROBE_PASS");
}

static void prepare_directory_persistence(void)
{
	struct stat st;
	int fd;

	rmdir(DIR_PATH);
	require(mkdir(DIR_PATH, 0755) == 0, "mkdir persistence");
	require(chmod(DIR_PATH, 0711) == 0, "chmod directory");
	st = path_stat(DIR_PATH);
	require(permission_bits(&st) == 0711, "directory cached mode");
	fd = open(DIR_PATH, O_RDONLY | O_DIRECTORY);
	require(fd >= 0, "open directory");
	require(fsync(fd) == 0, "fsync directory");
	require(close(fd) == 0, "close directory");
	puts("FS_METADATA_PREPARE_PASS mode=711");
}

static void verify_directory_persistence(void)
{
	struct stat st = path_stat(DIR_PATH);
	require(permission_bits(&st) == 0711, "directory chmod persistence");
	puts("FS_METADATA_DIRECTORY_PERSISTENCE_PASS mode=711");
	require(rmdir(DIR_PATH) == 0, "rmdir persistence");
	puts("FS_METADATA_VERIFY_PASS");
}

int main(int argc, char **argv)
{
	const char *mode = argc > 1 ? argv[1] : "normal";

	if (!strcmp(mode, "normal"))
		run_normal();
	else if (!strcmp(mode, "prepare"))
		prepare_directory_persistence();
	else if (!strcmp(mode, "verify"))
		verify_directory_persistence();
	else if (!strcmp(mode, "cleanup"))
		cleanup();
	else {
		fprintf(stderr,
			"usage: fs_metadata_probe_linux [normal|prepare|verify|cleanup]\n");
		return 2;
	}
	return 0;
}
