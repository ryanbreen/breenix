#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <spawn.h>
#include <signal.h>
#include <fcntl.h>
#include <stdlib.h>
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

/* Spawn must run on the guest: a scripted fork return cannot prove this path. */
static void spawn_handler(int sig) { (void)sig; }

int cmd_spawn_child(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    sigset_t mask;
    struct sigaction action;
    char data[4] = {0};
    char cwd[16];
    if (sigprocmask(SIG_SETMASK, NULL, &mask) || !sigismember(&mask, SIGUSR1)
        || sigaction(SIGUSR2, NULL, &action) || action.sa_handler != SIG_DFL
        || sigaction(SIGUSR1, NULL, &action) || action.sa_handler != SIG_DFL
        || !getcwd(cwd, sizeof cwd) || strcmp(cwd, "/tmp")
        || read(21, data, 3) != 3 || strcmp(data, "sdk")) return 1;
    return 23;
}

int cmd_spawn(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    posix_spawnattr_t attr;
    posix_spawn_file_actions_t actions;
    sigset_t mask, before, after;
    struct sigaction caught = {0}, ignored = {0};
    pid_t pid;
    int fd = open("/tmp/sdk-spawn-data", O_CREAT | O_TRUNC | O_RDWR, 0600);
    int dir = open("/tmp", O_RDONLY | O_DIRECTORY);
    if (fd < 0 || dir < 0 || write(fd, "sdk", 3) != 3) return 1;
    caught.sa_handler = spawn_handler;
    ignored.sa_handler = SIG_IGN;
    if (sigaction(SIGUSR1, &caught, NULL) || sigaction(SIGUSR2, &ignored, NULL)
        || sigprocmask(SIG_SETMASK, NULL, &before)) return 1;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    if (posix_spawnattr_init(&attr) || posix_spawn_file_actions_init(&actions)
        || posix_spawnattr_setsigmask(&attr, &mask)) return 1;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR2);
    if (posix_spawnattr_setsigdefault(&attr, &mask)
        || posix_spawnattr_setflags(&attr, POSIX_SPAWN_RESETIDS | POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK)
        || posix_spawn_file_actions_addchdir(&actions, "/")
        || posix_spawn_file_actions_addfchdir(&actions, dir)
        || posix_spawn_file_actions_addopen(&actions, 20, "sdk-spawn-data", O_RDONLY, 0)
        || posix_spawn_file_actions_adddup2(&actions, 20, 21)
        || posix_spawn_file_actions_adddup2(&actions, 21, 21)
        || posix_spawn_file_actions_addclose(&actions, 20)) return 1;
    char *child[] = {(char *)app_path, "spawn-child", NULL};
    char *env[] = {NULL};
    if (posix_spawn(&pid, app_path, &actions, &attr, child, env)
        || reap(pid, "spawn child") != 23
        || sigprocmask(SIG_SETMASK, NULL, &after) || before != after) return 1;
    posix_spawn_file_actions_destroy(&actions);
    posix_spawnattr_destroy(&attr);
    if (posix_spawn(&pid, "/no-such-sdk-program", NULL, NULL, child, env)
        || reap(pid, "missing spawn") != 127) return 1;
    char directory[4096];
    char *exit3[] = {(char *)app_path, "exit", "3", NULL};
    const char *base = strrchr(app_path, '/');
    if (!base || (size_t)(base - app_path) >= sizeof directory) return 1;
    memcpy(directory, app_path, (size_t)(base - app_path));
    directory[base - app_path] = 0;
    if (setenv("PATH", directory, 1)
        || posix_spawnp(&pid, base + 1, NULL, NULL, exit3, env)
        || reap(pid, "spawnp child") != 3) return 1;
    if (setenv("PATH", "/no-such-sdk-dir", 1)
        || posix_spawnp(&pid, "no-such-sdk-program", NULL, NULL, child, env)
        || reap(pid, "missing spawnp") != 127) return 1;
    close(fd);
    close(dir);
    unlink("/tmp/sdk-spawn-data");
    puts("spawn actions, attributes, exec and status 127 passed");
    return 0;
}
