#define _POSIX_C_SOURCE 200809L

#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "app.h"

static volatile sig_atomic_t usr1_count;
static volatile sig_atomic_t usr2_count;

static void on_usr1(int sig)
{
    if (sig == SIGUSR1) {
        usr1_count++;
    }
}

static void on_usr2(int sig)
{
    if (sig == SIGUSR2) {
        usr2_count++;
    }
}

static int install(int sig, void (*handler)(int))
{
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_handler = handler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART;
    return sigaction(sig, &action, NULL);
}

int cmd_signal_handler(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    if (install(SIGUSR1, on_usr1) != 0) {
        return failed("sigaction");
    }
    if (raise(SIGUSR1) != 0) {
        return failed("raise");
    }
    printf("count after raise=%d\n", (int)usr1_count);
    if (kill(getpid(), SIGUSR1) != 0) {
        return failed("kill");
    }
    printf("count after kill=%d\n", (int)usr1_count);
    return 0;
}

int cmd_signal_mask(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    if (install(SIGUSR2, on_usr2) != 0) {
        return failed("sigaction");
    }
    sigset_t block, pending;
    sigemptyset(&block);
    sigaddset(&block, SIGUSR2);
    if (sigprocmask(SIG_BLOCK, &block, NULL) != 0) {
        return failed("sigprocmask block");
    }
    raise(SIGUSR2);
    sigemptyset(&pending);
    if (sigpending(&pending) != 0) {
        return failed("sigpending");
    }
    printf("pending=%d delivered=%d\n", sigismember(&pending, SIGUSR2), (int)usr2_count);
    if (sigprocmask(SIG_UNBLOCK, &block, NULL) != 0) {
        return failed("sigprocmask unblock");
    }
    printf("delivered=%d\n", (int)usr2_count);
    return 0;
}
