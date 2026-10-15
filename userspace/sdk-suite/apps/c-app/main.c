#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <stdio.h>
#include <string.h>

#include "app.h"

struct command {
    const char *name;
    int (*run)(int argc, char **argv);
};

static const struct command commands[] = {
    {"hello", cmd_hello},
    {"args", cmd_args},
    {"exit", cmd_exit},
    {"stdio-format", cmd_stdio_format},
    {"stdio-stream", cmd_stdio_stream},
    {"stderr", cmd_stderr},
    {"files", cmd_files},
    {"dirs", cmd_dirs},
    {"fork-wait", cmd_fork_wait},
    {"exec", cmd_exec},
    {"pipe", cmd_pipe},
    {"signal-handler", cmd_signal_handler},
    {"signal-mask", cmd_signal_mask},
    {"threads", cmd_threads},
    {"mutex", cmd_mutex},
    {"condvar", cmd_condvar},
    {"socket-unix", cmd_socket_unix},
    {"socket-tcp", cmd_socket_tcp},
    {"socket-udp", cmd_socket_udp},
    {"math", cmd_math},
};

const char *app_path;

int failed(const char *what)
{
    printf("%s: %s\n", what, strerror(errno));
    return 1;
}

int main(int argc, char **argv)
{
    app_path = argv[0];
    if (argc < 2) {
        printf("usage: c-app COMMAND [ARG...]\n");
        return 2;
    }
    for (size_t i = 0; i < sizeof commands / sizeof commands[0]; i++) {
        if (strcmp(argv[1], commands[i].name) == 0) {
            return commands[i].run(argc - 1, argv + 1);
        }
    }
    printf("unknown command %s\n", argv[1]);
    return 2;
}
