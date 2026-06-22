#include <errno.h>
#include <limits.h>
#include <sys/stat.h>
#include <unistd.h>

static long pathconf_value(int name)
{
	switch (name) {
	case _PC_LINK_MAX:
		return _POSIX_LINK_MAX;
	case _PC_MAX_CANON:
		return _POSIX_MAX_CANON;
	case _PC_MAX_INPUT:
		return _POSIX_MAX_INPUT;
	case _PC_NAME_MAX:
		return NAME_MAX;
	case _PC_PATH_MAX:
		return PATH_MAX;
	case _PC_PIPE_BUF:
		return PIPE_BUF;
	case _PC_CHOWN_RESTRICTED:
		return _POSIX_CHOWN_RESTRICTED;
	case _PC_NO_TRUNC:
		return _POSIX_NO_TRUNC;
	case _PC_VDISABLE:
		return _POSIX_VDISABLE;
	case _PC_SYNC_IO:
	case _PC_ASYNC_IO:
	case _PC_PRIO_IO:
	case _PC_SOCK_MAXBUF:
		return -1;
	case _PC_FILESIZEBITS:
		return 64;
	case _PC_REC_INCR_XFER_SIZE:
	case _PC_REC_MAX_XFER_SIZE:
	case _PC_REC_MIN_XFER_SIZE:
	case _PC_REC_XFER_ALIGN:
	case _PC_ALLOC_SIZE_MIN:
		return -1;
	case _PC_SYMLINK_MAX:
		return _POSIX_SYMLINK_MAX;
	case _PC_2_SYMLINKS:
		return 1;
	default:
		errno = EINVAL;
		return -1;
	}
}

static int pathconf_name_valid(int name)
{
	return name >= _PC_LINK_MAX && name <= _PC_2_SYMLINKS;
}

long fpathconf(int fd, int name)
{
	struct stat st;

	if (fstat(fd, &st) < 0)
		return -1;

	if (!pathconf_name_valid(name)) {
		errno = EINVAL;
		return -1;
	}

	return pathconf_value(name);
}

long pathconf(const char *path, int name)
{
	struct stat st;

	if (!pathconf_name_valid(name)) {
		errno = EINVAL;
		return -1;
	}

	if (stat(path, &st) < 0)
		return -1;

	return pathconf_value(name);
}
