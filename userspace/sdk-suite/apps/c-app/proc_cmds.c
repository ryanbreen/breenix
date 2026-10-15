#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "app.h"

/* Wait for pid and print how it ended; returns its exit status or -1. */
static int reap(pid_t pid, const char *who)
{
    int status;
    if (waitpid(pid, &status, 0) != pid) {
        failed("waitpid");
        return -1;
    }
    if (!WIFEXITED(status)) {
        printf("%s did not exit normally (wait status %#x)\n", who, status);
        return -1;
    }
    return WEXITSTATUS(status);
}

int cmd_fork_wait(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        return failed("fork");
    }
    if (pid == 0) {
        _exit(7);
    }
    int code = reap(pid, "the child");
    if (code < 0) {
        return 1;
    }
    printf("child exited %d\n", code);
    return 0;
}

int cmd_exec(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        return failed("fork");
    }
    if (pid == 0) {
        char *const child_argv[] = {(char *)app_path, "exit", "3", NULL};
        execv(app_path, child_argv);
        _exit(127);
    }
    int code = reap(pid, "the exec'd child");
    if (code < 0) {
        return 1;
    }
    printf("exec child exited %d\n", code);
    return 0;
}

int cmd_pipe(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    int fds[2];
    if (pipe(fds) != 0) {
        return failed("pipe");
    }
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        return failed("fork");
    }
    if (pid == 0) {
        close(fds[0]);
        const char *text = "through the pipe";
        _exit(write(fds[1], text, strlen(text)) == (ssize_t)strlen(text) ? 0 : 1);
    }
    close(fds[1]);
    char buf[64];
    size_t got = 0;
    ssize_t n;
    while ((n = read(fds[0], buf + got, sizeof buf - 1 - got)) > 0) {
        got += (size_t)n;
    }
    buf[got] = '\0';
    close(fds[0]);
    if (reap(pid, "the writer") != 0) {
        return 1;
    }
    printf("pipe: %s\n", buf);
    return 0;
}
