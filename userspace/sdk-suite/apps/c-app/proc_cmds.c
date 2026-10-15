#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <spawn.h>
#include <fcntl.h>
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

/* Exercise the packaged spawn ABI and real fork/attribute/action/exec sequence.
 * Keep the original execv check and its exact output below. */
static int check_spawn(void)
{
    _Static_assert(sizeof(posix_spawnattr_t) == 32, "spawn attribute ABI");
    _Static_assert(sizeof(posix_spawn_file_actions_t) == 16, "spawn actions ABI");
    posix_spawnattr_t attr;
    posix_spawn_file_actions_t actions;
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    if (posix_spawnattr_init(&attr) || posix_spawn_file_actions_init(&actions)
        || posix_spawnattr_setsigmask(&attr, &mask)
        || posix_spawnattr_setsigdefault(&attr, &mask)
        || posix_spawnattr_setflags(&attr, POSIX_SPAWN_RESETIDS | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF)
        || posix_spawn_file_actions_addopen(&actions, 9, "/dev/null", O_RDONLY, 0)
        || posix_spawn_file_actions_adddup2(&actions, 9, 10)
        || posix_spawn_file_actions_addclose(&actions, 9)
        || posix_spawn_file_actions_addclose(&actions, 10)
        || posix_spawn_file_actions_addchdir(&actions, ".")) {
        return 1;
    }
    char *const args[] = {(char *)app_path, "exit", "3", NULL};
    char *const env[] = {"SPAWN_TEST=yes", NULL};
    pid_t pid;
    int error = posix_spawn(&pid, app_path, &actions, &attr, args, env);
    posix_spawn_file_actions_destroy(&actions);
    posix_spawnattr_destroy(&attr);
    if (error || reap(pid, "spawn child") != 3) return 1;
    char directory[4096];
    const char *base = strrchr(app_path, '/');
    if (!base || (size_t)(base - app_path) >= sizeof directory) return 1;
    memcpy(directory, app_path, (size_t)(base - app_path));
    directory[base - app_path] = 0;
    if (setenv("PATH", directory, 1)) return 1;
    error = posix_spawnp(&pid, base + 1, NULL, NULL, args, env);
    if (error || reap(pid, "spawnp child") != 3) return 1;
    error = posix_spawn(&pid, "/sdk-no-such-program", NULL, NULL, args, env);
    return error || reap(pid, "failed spawn exec") != 127;
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
    if (check_spawn()) return 1;
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
