/* N1570 §6.2.4; POSIX fork, execv and waitpid. */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#ifdef __clang__
#include <sys/wait.h>
#include <unistd.h>
#else
/* The Breenix POSIX interfaces used here, within the bcc 25.7 subset. */
typedef int pid_t;
pid_t fork(void);
int execv(const char *, char *const[]);
void _exit(int);
pid_t waitpid(pid_t, int *, int);

/* Breenix <sys/wait.h> encodes a normal exit with code zero as zero. */
#define WIFEXITED(status) (((status) & 0x7f) == 0)
#define WEXITSTATUS(status) (((status) & 0xff00) >> 8)
#endif

static _Thread_local volatile int initialized = 17;
static _Thread_local volatile int zero_fill;

int main(int argc, char **argv)
{
    if (initialized != 17 || zero_fill != 0)
        return 1;
    if (argc == 2 && argv[1][0] == 'e')
        return 0;

    initialized = 41;
    zero_fill = 42;
    pid_t child = fork();
    if (child == 0) {
        char *exec_args[3];
        if (initialized != 41 || zero_fill != 42)
            _exit(2);
        initialized = 61;
        zero_fill = 62;
        exec_args[0] = argv[0];
        exec_args[1] = "exec";
        exec_args[2] = 0;
        execv(argv[0], exec_args);
        _exit(3);
    }
    if (child < 0)
        return 4;

    int status;
    pid_t reaped;
    do {
        reaped = waitpid(child, &status, 0);
    } while (reaped < 0 && errno == EINTR);
    if (reaped != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
        return 5;
    return initialized != 41 || zero_fill != 42;
}
