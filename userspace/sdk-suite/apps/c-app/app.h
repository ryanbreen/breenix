/* The SDK suite's C app: one program, one subcommand per guest case.
 *
 * Each subcommand prints fixed lines on stdout and exits 0 when the behaviour it
 * exercises is right; the guest suite compares the exact output and exit status.
 * A subcommand that sees something wrong prints what it saw instead, so the case's
 * failure message shows it, and exits 1. */
#ifndef SDK_C_APP_H
#define SDK_C_APP_H

int cmd_hello(int argc, char **argv);
int cmd_args(int argc, char **argv);
int cmd_exit(int argc, char **argv);
int cmd_stdio_format(int argc, char **argv);
int cmd_stdio_stream(int argc, char **argv);
int cmd_stderr(int argc, char **argv);
int cmd_files(int argc, char **argv);
int cmd_dirs(int argc, char **argv);
int cmd_fork_wait(int argc, char **argv);
int cmd_spawn(int argc, char **argv);
int cmd_spawn_child(int argc, char **argv);
int cmd_exec(int argc, char **argv);
int cmd_pipe(int argc, char **argv);
int cmd_signal_handler(int argc, char **argv);
int cmd_signal_mask(int argc, char **argv);
int cmd_threads(int argc, char **argv);
int cmd_mutex(int argc, char **argv);
int cmd_condvar(int argc, char **argv);
int cmd_socket_unix(int argc, char **argv);
int cmd_socket_tcp(int argc, char **argv);
int cmd_socket_udp(int argc, char **argv);
int cmd_math(int argc, char **argv);

/* The path this program was started by (its argv[0]). */
extern const char *app_path;

/* Print "<what>: <strerror(errno)>" and return 1. */
int failed(const char *what);

#endif
