#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

static volatile sig_atomic_t orphan_hup_seen;

static void orphan_hup_handler(int signo)
{
    (void)signo;
    orphan_hup_seen = 1;
}

static void child_contract(const char *slave_name)
{
    assert(setsid() == getpid());
    int tty = open(slave_name, O_RDWR | O_NOCTTY);
    assert(tty >= 0);
    assert(ioctl(tty, TIOCSCTTY, 0) == 0);
    assert(ioctl(tty, TIOCSCTTY, 0) == 0);

    pid_t sid = -1;
    pid_t pgrp = -1;
    assert(ioctl(tty, TIOCGSID, &sid) == 0);
    assert(ioctl(tty, TIOCGPGRP, &pgrp) == 0);
    assert(sid == getsid(0));
    assert(pgrp == getpgrp());

    int ready[2];
    assert(pipe(ready) == 0);
    pid_t member = fork();
    assert(member >= 0);
    if (member == 0) {
        close(ready[0]);
        assert(setpgid(0, 0) == 0);
        assert(write(ready[1], "x", 1) == 1);
        for (;;)
            pause();
    }

    close(ready[1]);
    char byte;
    assert(read(ready[0], &byte, 1) == 1);
    signal(SIGTTOU, SIG_IGN);
    signal(SIGHUP, SIG_IGN);
    assert(ioctl(tty, TIOCSPGRP, &member) == 0);
    assert(ioctl(tty, TIOCGPGRP, &pgrp) == 0);
    assert(pgrp == member);

    pid_t stopped_reader = fork();
    assert(stopped_reader >= 0);
    if (stopped_reader == 0) {
        assert(setpgid(0, 0) == 0);
        ssize_t unexpected = read(tty, &byte, 1);
        _exit(unexpected < 0 ? 1 : 2);
    }
    int stopped_status = 0;
    assert(waitpid(stopped_reader, &stopped_status, WUNTRACED) == stopped_reader);
    assert(WIFSTOPPED(stopped_status) && WSTOPSIG(stopped_status) == SIGTTIN);
    assert(ioctl(tty, TIOCSPGRP, &stopped_reader) == 0);
    assert(kill(stopped_reader, SIGCONT) == 0);
    assert(waitpid(stopped_reader, &stopped_status, WCONTINUED) == stopped_reader);
    assert(WIFCONTINUED(stopped_status));
    assert(kill(stopped_reader, SIGKILL) == 0);
    assert(waitpid(stopped_reader, NULL, 0) == stopped_reader);
    assert(ioctl(tty, TIOCSPGRP, &member) == 0);

    signal(SIGTTIN, SIG_IGN);
    errno = 0;
    assert(read(tty, &byte, 1) == -1);
    assert(errno == EIO);

    struct termios old_termios;
    struct termios termios;
    assert(tcgetattr(tty, &old_termios) == 0);
    termios = old_termios;
    termios.c_lflag |= TOSTOP;
    assert(tcsetattr(tty, TCSANOW, &termios) == 0);
    memset(&termios, 0, sizeof(termios));
    assert(tcgetattr(tty, &termios) == 0);
    assert((termios.c_lflag & TOSTOP) != 0);
    assert(write(tty, "J\n", 2) == 2);
    assert(tcsetattr(tty, TCSANOW, &old_termios) == 0);

    pid_t invalid = 0x3fffffff;
    errno = 0;
    assert(ioctl(tty, TIOCSPGRP, &invalid) == -1);
    assert(errno == ESRCH);

    pid_t self_group = getpgrp();
    assert(ioctl(tty, TIOCSPGRP, &self_group) == 0);
    assert(ioctl(tty, TIOCNOTTY, 0) == 0);
    errno = 0;
    assert(ioctl(tty, TIOCGPGRP, &pgrp) == -1);
    assert(errno == ENOTTY);

    kill(member, SIGKILL);
    assert(waitpid(member, NULL, 0) == member);
    close(tty);
    puts("JOB_CONTROL_LINUX controlling_tty_foreground PASS");
    puts("JOB_CONTROL_LINUX background_stop_continue PASS");
    puts("JOB_CONTROL_LINUX background_read_eio PASS");
    puts("JOB_CONTROL_LINUX termios_tostop_ignored PASS");
}

static void orphaned_stopped_group_contract(void)
{
    assert(setsid() == getpid());
    int report[2];
    assert(pipe(report) == 0);
    pid_t bridge = fork();
    assert(bridge >= 0);
    if (bridge == 0) {
        pid_t stopped = fork();
        assert(stopped >= 0);
        if (stopped == 0) {
            assert(setpgid(0, 0) == 0);
            struct sigaction action;
            memset(&action, 0, sizeof(action));
            action.sa_handler = orphan_hup_handler;
            sigemptyset(&action.sa_mask);
            assert(sigaction(SIGHUP, &action, NULL) == 0);
            assert(kill(getpid(), SIGSTOP) == 0);
            assert(orphan_hup_seen == 1);
            assert(write(report[1], "h", 1) == 1);
            _exit(0);
        }
        int status = 0;
        assert(waitpid(stopped, &status, WUNTRACED) == stopped);
        assert(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP);
        _exit(0);
    }

    char byte = 0;
    assert(read(report[0], &byte, 1) == 1 && byte == 'h');
    int status = 0;
    assert(waitpid(bridge, &status, 0) == bridge);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    close(report[0]);
    close(report[1]);
    puts("JOB_CONTROL_LINUX orphan_hup_cont PASS");
}

int main(void)
{
    setbuf(stdout, NULL);
    int master = posix_openpt(O_RDWR | O_NOCTTY);
    assert(master >= 0);
    assert(grantpt(master) == 0);
    assert(unlockpt(master) == 0);
    char *slave_name = ptsname(master);
    assert(slave_name != NULL);

    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        child_contract(slave_name);
        _exit(0);
    }

    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    close(master);
    pid_t orphan = fork();
    assert(orphan >= 0);
    if (orphan == 0) {
        orphaned_stopped_group_contract();
        _exit(0);
    }
    assert(waitpid(orphan, &status, 0) == orphan);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    puts("JOB_CONTROL_LINUX ALL PASS");
    return 0;
}
