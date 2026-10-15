/* Functions and constants an ordinary POSIX application uses, at least one from each
 * major header. The SDK's headers must declare them all and its libc must define every
 * function: the harness compiles this file and links it into a program. */
#define _POSIX_C_SOURCE 202405L

#include <arpa/inet.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fnmatch.h>
#include <glob.h>
#include <grp.h>
#include <langinfo.h>
#include <locale.h>
#include <math.h>
#include <netdb.h>
#include <poll.h>
#include <pthread.h>
#include <pwd.h>
#include <regex.h>
#include <sched.h>
#include <semaphore.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>
#include <wchar.h>

/* The addresses are stored so the linker must resolve every function. */
void *volatile functions[] = {
    (void *)open, (void *)read, (void *)write, (void *)close, (void *)lseek,
    (void *)fork, (void *)execv, (void *)execvp, (void *)waitpid, (void *)pipe,
    (void *)dup2, (void *)getcwd, (void *)chdir, (void *)isatty, (void *)sysconf,
    (void *)opendir, (void *)readdir, (void *)closedir,
    (void *)stat, (void *)fstat, (void *)mkdir, (void *)chmod, (void *)umask,
    (void *)sigaction, (void *)sigprocmask, (void *)kill, (void *)raise,
    (void *)pthread_create, (void *)pthread_join, (void *)pthread_mutex_lock,
    (void *)pthread_cond_wait, (void *)sem_init, (void *)sched_yield,
    (void *)socket, (void *)bind, (void *)listen, (void *)accept, (void *)connect,
    (void *)sendto, (void *)recvfrom, (void *)getsockname, (void *)socketpair,
    (void *)inet_pton, (void *)getaddrinfo, (void *)poll, (void *)select,
    (void *)mmap, (void *)munmap, (void *)clock_gettime, (void *)nanosleep,
    (void *)getrlimit, (void *)uname, (void *)tcgetattr, (void *)fnmatch,
    (void *)glob, (void *)regcomp, (void *)getpwuid, (void *)getgrgid,
    (void *)posix_spawn, (void *)writev, (void *)mkstemp, (void *)realpath,
    (void *)strtok_r, (void *)strdup, (void *)getline, (void *)snprintf,
    (void *)qsort, (void *)malloc, (void *)setenv, (void *)getopt,
    (void *)setlocale, (void *)nl_langinfo, (void *)wcslen, (void *)mbrtowc,
    (void *)sqrt, (void *)sin, (void *)pow, (void *)strerror,
};

int constants[] = {
    O_CLOEXEC, O_NONBLOCK, EINTR, EAGAIN, SIGCHLD, SA_RESTART, AF_UNIX, AF_INET,
    SOCK_STREAM, SOCK_DGRAM, POLLIN, PROT_READ, MAP_PRIVATE, WNOHANG, CLOCK_MONOTONIC,
    PTHREAD_MUTEX_RECURSIVE, RLIMIT_NOFILE, FNM_NOMATCH, REG_EXTENDED, TCSANOW,
};

int main(void)
{
    return functions[0] == 0 || constants[0] == 0;
}
