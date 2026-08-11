#define _GNU_SOURCE

#include <assert.h>
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

typedef uint64_t kernel_sigset_t;

static kernel_sigset_t signal_bit(int signo)
{
    return UINT64_C(1) << (signo - 1);
}

static void test_query_ignores_how(void)
{
    kernel_sigset_t oldset = UINT64_MAX;
    assert(syscall(SYS_rt_sigprocmask, -1, NULL, &oldset, sizeof(oldset)) == 0);
    puts("SIGNAL_PHASE5_LINUX sigprocmask_query PASS");
}

static void test_sigaction_size_and_input_order(void)
{
    struct sigaction old_action;
    struct sigaction snapshot;
    memset(&old_action, 0x5a, sizeof(old_action));
    snapshot = old_action;

    errno = 0;
    assert(syscall(SYS_rt_sigaction, SIGUSR2, NULL, NULL, 0) == -1);
    assert(errno == EINVAL);

    errno = 0;
    assert(syscall(SYS_rt_sigaction, SIGUSR2, (void *)(uintptr_t)-1,
                   &old_action, sizeof(kernel_sigset_t)) == -1);
    assert(errno == EFAULT);
    assert(memcmp(&old_action, &snapshot, sizeof(old_action)) == 0);
    puts("SIGNAL_PHASE5_LINUX sigaction_validation PASS");
}

static void test_sigqueueinfo_null_signal(void)
{
    siginfo_t info;
    memset(&info, 0, sizeof(info));
    info.si_code = SI_QUEUE;
    info.si_pid = getpid();
    info.si_uid = getuid();
    assert(syscall(SYS_rt_sigqueueinfo, getpid(), 0, &info) == 0);
    puts("SIGNAL_PHASE5_LINUX sigqueueinfo_zero PASS");
}

static void exec_pending_target(void)
{
    kernel_sigset_t mask = 0;
    kernel_sigset_t pending = 0;
    assert(syscall(SYS_rt_sigprocmask, -1, NULL, &mask, sizeof(mask)) == 0);
    assert(syscall(SYS_rt_sigpending, &pending, sizeof(pending)) == 0);
    assert((mask & signal_bit(SIGUSR1)) != 0);
    assert((pending & signal_bit(SIGUSR1)) != 0);
    puts("SIGNAL_PHASE5_LINUX exec_pending PASS");
    puts("SIGNAL_PHASE5_LINUX ALL PASS");
}

static void test_pending_survives_exec(char *self)
{
    kernel_sigset_t block = signal_bit(SIGUSR1);
    kernel_sigset_t pending = 0;
    assert(syscall(SYS_rt_sigprocmask, SIG_BLOCK, &block, NULL, sizeof(block)) == 0);
    assert(kill(getpid(), SIGUSR1) == 0);
    assert(syscall(SYS_rt_sigpending, &pending, sizeof(pending)) == 0);
    assert((pending & block) != 0);

    char *const argv[] = {self, (char *)"--exec-target", NULL};
    execv(self, argv);
    assert(!"execv failed");
}

int main(int argc, char **argv)
{
    setbuf(stdout, NULL);
    if (argc == 2 && strcmp(argv[1], "--exec-target") == 0) {
        exec_pending_target();
        return 0;
    }

    assert(argc >= 1);
    test_query_ignores_how();
    test_sigaction_size_and_input_order();
    test_sigqueueinfo_null_signal();
    test_pending_survives_exec(argv[0]);
    return 1;
}
