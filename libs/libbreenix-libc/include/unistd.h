/* unistd.h: POSIX.1-2024 standard symbolic constants and types. */
#ifndef _UNISTD_H
#define _UNISTD_H

#include <__breenix/types.h>
#define __need_size_t
#define __need_NULL
#include <stddef.h>

typedef __breenix_ssize_t ssize_t;
typedef __breenix_off_t off_t;
typedef __breenix_pid_t pid_t;
typedef __breenix_uid_t uid_t;
typedef __breenix_gid_t gid_t;
typedef __breenix_useconds_t useconds_t;
typedef __INTPTR_TYPE__ intptr_t;

#define _POSIX_VERSION 202405L
#define _POSIX2_VERSION 202405L
#define _XOPEN_VERSION 800
#define _POSIX_THREADS 202405L
#define _POSIX_TIMEOUTS 202405L
#define _POSIX_TIMERS 202405L
#define _POSIX_MONOTONIC_CLOCK 202405L
#define _POSIX_SEMAPHORES 202405L
#define _POSIX_SPIN_LOCKS 202405L
#define _POSIX_BARRIERS 202405L
#define _POSIX_READER_WRITER_LOCKS 202405L
#define _POSIX_THREAD_SAFE_FUNCTIONS 202405L
#define _POSIX_MAPPED_FILES 202405L
#define _POSIX_SHARED_MEMORY_OBJECTS 202405L
#define _POSIX_MESSAGE_PASSING 202405L
#define _POSIX_JOB_CONTROL 1
#define _POSIX_SAVED_IDS 1
#define _POSIX_NO_TRUNC 1
#define _POSIX_VDISABLE '\0'
#define _POSIX_CHOWN_RESTRICTED 1

#define STDIN_FILENO 0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

#define F_OK 0
#define X_OK 1
#define W_OK 2
#define R_OK 4

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define SEEK_DATA 3
#define SEEK_HOLE 4

#define F_ULOCK 0
#define F_LOCK 1
#define F_TLOCK 2
#define F_TEST 3

/* pathconf and fpathconf names (Linux values). */
#define _PC_LINK_MAX 0
#define _PC_MAX_CANON 1
#define _PC_MAX_INPUT 2
#define _PC_NAME_MAX 3
#define _PC_PATH_MAX 4
#define _PC_PIPE_BUF 5
#define _PC_CHOWN_RESTRICTED 6
#define _PC_NO_TRUNC 7
#define _PC_VDISABLE 8

/* sysconf names (Linux values). */
#define _SC_ARG_MAX 0
#define _SC_CHILD_MAX 1
#define _SC_CLK_TCK 2
#define _SC_NGROUPS_MAX 3
#define _SC_OPEN_MAX 4
#define _SC_STREAM_MAX 5
#define _SC_TZNAME_MAX 6
#define _SC_JOB_CONTROL 7
#define _SC_SAVED_IDS 8
#define _SC_VERSION 29
#define _SC_PAGESIZE 30
#define _SC_PAGE_SIZE _SC_PAGESIZE
#define _SC_RTSIG_MAX 31
#define _SC_SEM_NSEMS_MAX 32
#define _SC_SEM_VALUE_MAX 33
#define _SC_SIGQUEUE_MAX 34
#define _SC_TIMER_MAX 35
#define _SC_LINE_MAX 43
#define _SC_GETGR_R_SIZE_MAX 69
#define _SC_GETPW_R_SIZE_MAX 70
#define _SC_LOGIN_NAME_MAX 71
#define _SC_TTY_NAME_MAX 72
#define _SC_THREADS 67
#define _SC_THREAD_DESTRUCTOR_ITERATIONS 73
#define _SC_THREAD_KEYS_MAX 74
#define _SC_THREAD_STACK_MIN 75
#define _SC_THREAD_THREADS_MAX 76
#define _SC_NPROCESSORS_CONF 83
#define _SC_NPROCESSORS_ONLN 84
#define _SC_PHYS_PAGES 85
#define _SC_AVPHYS_PAGES 86
#define _SC_HOST_NAME_MAX 180
#define _SC_MONOTONIC_CLOCK 149

/* confstr names. */
#define _CS_PATH 0

__BREENIX_BEGIN_DECLS

int access(const char *, int);
unsigned alarm(unsigned);
int chdir(const char *);
int chown(const char *, uid_t, gid_t);
int close(int);
size_t confstr(int, char *, size_t);
int dup(int);
int dup2(int, int);
int dup3(int, int, int);
__BREENIX_NORETURN void _exit(int);
int execl(const char *, const char *, ...);
int execle(const char *, const char *, ...);
int execlp(const char *, const char *, ...);
int execv(const char *, char *const[]);
int execve(const char *, char *const[], char *const[]);
int execvp(const char *, char *const[]);
int faccessat(int, const char *, int, int);
int fchdir(int);
int fchown(int, uid_t, gid_t);
int fchownat(int, const char *, uid_t, gid_t, int);
int fdatasync(int);
int fexecve(int, char *const[], char *const[]);
pid_t fork(void);
long fpathconf(int, int);
int fsync(int);
int ftruncate(int, off_t);
char *getcwd(char *, size_t);
gid_t getegid(void);
uid_t geteuid(void);
gid_t getgid(void);
int getgroups(int, gid_t[]);
long gethostid(void);
int gethostname(char *, size_t);
char *getlogin(void);
int getlogin_r(char *, size_t);
int getopt(int, char *const[], const char *);
pid_t getpgid(pid_t);
pid_t getpgrp(void);
pid_t getpid(void);
pid_t getppid(void);
pid_t getsid(pid_t);
uid_t getuid(void);
int isatty(int);
int lchown(const char *, uid_t, gid_t);
int link(const char *, const char *);
int linkat(int, const char *, int, const char *, int);
int lockf(int, int, off_t);
off_t lseek(int, off_t, int);
int nice(int);
long pathconf(const char *, int);
int pause(void);
int pipe(int[2]);
int pipe2(int[2], int);
ssize_t pread(int, void *, size_t, off_t);
ssize_t pwrite(int, const void *, size_t, off_t);
ssize_t read(int, void *, size_t);
ssize_t readlink(const char *__BREENIX_RESTRICT, char *__BREENIX_RESTRICT, size_t);
ssize_t readlinkat(int, const char *__BREENIX_RESTRICT, char *__BREENIX_RESTRICT, size_t);
int rmdir(const char *);
int setegid(gid_t);
int seteuid(uid_t);
int setgid(gid_t);
int setpgid(pid_t, pid_t);
pid_t setpgrp(void);
int setregid(gid_t, gid_t);
int setreuid(uid_t, uid_t);
pid_t setsid(void);
int setuid(uid_t);
unsigned sleep(unsigned);
void swab(const void *__BREENIX_RESTRICT, void *__BREENIX_RESTRICT, ssize_t);
int symlink(const char *, const char *);
int symlinkat(const char *, int, const char *);
void sync(void);
long sysconf(int);
pid_t tcgetpgrp(int);
int tcsetpgrp(int, pid_t);
int truncate(const char *, off_t);
char *ttyname(int);
int ttyname_r(int, char *, size_t);
int unlink(const char *);
int unlinkat(int, const char *, int);
ssize_t write(int, const void *, size_t);

extern char *optarg;
extern int optind, opterr, optopt;

#if defined(__BREENIX_POSIX)
int getentropy(void *, size_t);
#endif

#ifdef __BREENIX_EXT
extern char **environ;
int getpagesize(void);
int getdtablesize(void);
int usleep(useconds_t);
useconds_t ualarm(useconds_t, useconds_t);
pid_t vfork(void);
int chroot(const char *);
int sethostname(const char *, size_t);
int setgroups(size_t, const gid_t *);
int getresuid(uid_t *, uid_t *, uid_t *);
int getresgid(gid_t *, gid_t *, gid_t *);
int setresuid(uid_t, uid_t, uid_t);
int setresgid(gid_t, gid_t, gid_t);
int execvpe(const char *, char *const[], char *const[]);
char *get_current_dir_name(void);
int syncfs(int);
long syscall(long, ...);
pid_t gettid(void);
#define lseek64 lseek
#define ftruncate64 ftruncate
#define truncate64 truncate
#define pread64 pread
#define pwrite64 pwrite
#endif

__BREENIX_END_DECLS

#endif
