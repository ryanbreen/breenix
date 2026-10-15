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
/* The programming environments: only LP64 with a 64-bit off_t. */
#define _POSIX_V8_ILP32_OFF32 (-1)
#define _POSIX_V8_ILP32_OFFBIG (-1)
#define _POSIX_V8_LP64_OFF64 1
#define _POSIX_V8_LPBIG_OFFBIG (-1)
#define _POSIX_V7_ILP32_OFF32 (-1)
#define _POSIX_V7_ILP32_OFFBIG (-1)
#define _POSIX_V7_LP64_OFF64 1
#define _POSIX_V7_LPBIG_OFFBIG (-1)

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

/* Reserved sequential query IDs selected by this implementation. */
#define _PC_LINK_MAX 0
#define _PC_MAX_CANON 1
#define _PC_MAX_INPUT 2
#define _PC_NAME_MAX 3
#define _PC_PATH_MAX 4
#define _PC_PIPE_BUF 5
#define _PC_CHOWN_RESTRICTED 6
#define _PC_NO_TRUNC 7
#define _PC_VDISABLE 8
#define _PC_SYNC_IO 9
#define _PC_ASYNC_IO 10
#define _PC_PRIO_IO 11
#define _PC_SOCK_MAXBUF 12
#define _PC_FILESIZEBITS 13
#define _PC_REC_INCR_XFER_SIZE 14
#define _PC_REC_MAX_XFER_SIZE 15
#define _PC_REC_MIN_XFER_SIZE 16
#define _PC_REC_XFER_ALIGN 17
#define _PC_ALLOC_SIZE_MIN 18
#define _PC_SYMLINK_MAX 19
#define _PC_2_SYMLINKS 20
#define _PC_TIMESTAMP_RESOLUTION 21
#define _PC_TEXTDOMAIN_MAX 22
#define _PC_FALLOC 23

/* sysconf names, numbered as Linux numbers them; the POSIX_V8 programming
 * environments, which Linux lacks, follow its last name. */
#define _SC_ARG_MAX 0
#define _SC_CHILD_MAX 1
#define _SC_CLK_TCK 2
#define _SC_NGROUPS_MAX 3
#define _SC_OPEN_MAX 4
#define _SC_STREAM_MAX 5
#define _SC_TZNAME_MAX 6
#define _SC_JOB_CONTROL 7
#define _SC_SAVED_IDS 8
#define _SC_REALTIME_SIGNALS 9
#define _SC_PRIORITY_SCHEDULING 10
#define _SC_TIMERS 11
#define _SC_ASYNCHRONOUS_IO 12
#define _SC_PRIORITIZED_IO 13
#define _SC_SYNCHRONIZED_IO 14
#define _SC_FSYNC 15
#define _SC_MAPPED_FILES 16
#define _SC_MEMLOCK 17
#define _SC_MEMLOCK_RANGE 18
#define _SC_MEMORY_PROTECTION 19
#define _SC_MESSAGE_PASSING 20
#define _SC_SEMAPHORES 21
#define _SC_SHARED_MEMORY_OBJECTS 22
#define _SC_AIO_LISTIO_MAX 23
#define _SC_AIO_MAX 24
#define _SC_AIO_PRIO_DELTA_MAX 25
#define _SC_DELAYTIMER_MAX 26
#define _SC_MQ_OPEN_MAX 27
#define _SC_MQ_PRIO_MAX 28
#define _SC_VERSION 29
#define _SC_PAGESIZE 30
#define _SC_PAGE_SIZE _SC_PAGESIZE
#define _SC_RTSIG_MAX 31
#define _SC_SEM_NSEMS_MAX 32
#define _SC_SEM_VALUE_MAX 33
#define _SC_SIGQUEUE_MAX 34
#define _SC_TIMER_MAX 35
#define _SC_BC_BASE_MAX 36
#define _SC_BC_DIM_MAX 37
#define _SC_BC_SCALE_MAX 38
#define _SC_BC_STRING_MAX 39
#define _SC_COLL_WEIGHTS_MAX 40
#define _SC_EXPR_NEST_MAX 42
#define _SC_LINE_MAX 43
#define _SC_RE_DUP_MAX 44
#define _SC_2_VERSION 46
#define _SC_2_C_BIND 47
#define _SC_2_C_DEV 48
#define _SC_2_FORT_DEV 49
#define _SC_2_FORT_RUN 50
#define _SC_2_SW_DEV 51
#define _SC_2_LOCALEDEF 52
#define _SC_IOV_MAX 60
#define _SC_THREADS 67
#define _SC_THREAD_SAFE_FUNCTIONS 68
#define _SC_GETGR_R_SIZE_MAX 69
#define _SC_GETPW_R_SIZE_MAX 70
#define _SC_LOGIN_NAME_MAX 71
#define _SC_TTY_NAME_MAX 72
#define _SC_THREAD_DESTRUCTOR_ITERATIONS 73
#define _SC_THREAD_KEYS_MAX 74
#define _SC_THREAD_STACK_MIN 75
#define _SC_THREAD_THREADS_MAX 76
#define _SC_THREAD_ATTR_STACKADDR 77
#define _SC_THREAD_ATTR_STACKSIZE 78
#define _SC_THREAD_PRIORITY_SCHEDULING 79
#define _SC_THREAD_PRIO_INHERIT 80
#define _SC_THREAD_PRIO_PROTECT 81
#define _SC_THREAD_PROCESS_SHARED 82
#define _SC_NPROCESSORS_CONF 83
#define _SC_NPROCESSORS_ONLN 84
#define _SC_PHYS_PAGES 85
#define _SC_AVPHYS_PAGES 86
#define _SC_ATEXIT_MAX 87
#define _SC_XOPEN_VERSION 89
#define _SC_XOPEN_UNIX 91
#define _SC_XOPEN_CRYPT 92
#define _SC_XOPEN_ENH_I18N 93
#define _SC_XOPEN_SHM 94
#define _SC_2_CHAR_TERM 95
#define _SC_2_UPE 97
#define _SC_NZERO 109
#define _SC_XOPEN_REALTIME 130
#define _SC_XOPEN_REALTIME_THREADS 131
#define _SC_ADVISORY_INFO 132
#define _SC_BARRIERS 133
#define _SC_CLOCK_SELECTION 137
#define _SC_CPUTIME 138
#define _SC_THREAD_CPUTIME 139
#define _SC_MONOTONIC_CLOCK 149
#define _SC_READER_WRITER_LOCKS 153
#define _SC_SPIN_LOCKS 154
#define _SC_REGEXP 155
#define _SC_SHELL 157
#define _SC_SPAWN 159
#define _SC_SPORADIC_SERVER 160
#define _SC_THREAD_SPORADIC_SERVER 161
#define _SC_TIMEOUTS 164
#define _SC_TYPED_MEMORY_OBJECTS 165
#define _SC_SYMLOOP_MAX 173
#define _SC_HOST_NAME_MAX 180
#define _SC_IPV6 235
#define _SC_RAW_SOCKETS 236
#define _SC_V7_ILP32_OFF32 237
#define _SC_V7_ILP32_OFFBIG 238
#define _SC_V7_LP64_OFF64 239
#define _SC_V7_LPBIG_OFFBIG 240
#define _SC_SS_REPL_MAX 241
#define _SC_THREAD_ROBUST_PRIO_INHERIT 247
#define _SC_THREAD_ROBUST_PRIO_PROTECT 248
#define _SC_V8_ILP32_OFF32 250
#define _SC_V8_ILP32_OFFBIG 251
#define _SC_V8_LP64_OFF64 252
#define _SC_V8_LPBIG_OFFBIG 253

/* confstr names: Linux's numbers for PATH and the POSIX_V7 environments,
 * then the POSIX_V8 names, which Linux lacks. */
#define _CS_PATH 0
#define _CS_POSIX_V7_WIDTH_RESTRICTED_ENVS 5
#define _CS_POSIX_V7_ILP32_OFF32_CFLAGS 1132
#define _CS_POSIX_V7_ILP32_OFF32_LDFLAGS 1133
#define _CS_POSIX_V7_ILP32_OFF32_LIBS 1134
#define _CS_POSIX_V7_ILP32_OFF32_LINTFLAGS 1135
#define _CS_POSIX_V7_ILP32_OFFBIG_CFLAGS 1136
#define _CS_POSIX_V7_ILP32_OFFBIG_LDFLAGS 1137
#define _CS_POSIX_V7_ILP32_OFFBIG_LIBS 1138
#define _CS_POSIX_V7_ILP32_OFFBIG_LINTFLAGS 1139
#define _CS_POSIX_V7_LP64_OFF64_CFLAGS 1140
#define _CS_POSIX_V7_LP64_OFF64_LDFLAGS 1141
#define _CS_POSIX_V7_LP64_OFF64_LIBS 1142
#define _CS_POSIX_V7_LP64_OFF64_LINTFLAGS 1143
#define _CS_POSIX_V7_LPBIG_OFFBIG_CFLAGS 1144
#define _CS_POSIX_V7_LPBIG_OFFBIG_LDFLAGS 1145
#define _CS_POSIX_V7_LPBIG_OFFBIG_LIBS 1146
#define _CS_POSIX_V7_LPBIG_OFFBIG_LINTFLAGS 1147
#define _CS_V7_ENV 1149
#define _CS_V8_ENV 1150
#define _CS_POSIX_V8_WIDTH_RESTRICTED_ENVS 1151
#define _CS_POSIX_V8_THREADS_CFLAGS 1152
#define _CS_POSIX_V8_THREADS_LDFLAGS 1153
#define _CS_POSIX_V7_THREADS_CFLAGS 1154
#define _CS_POSIX_V7_THREADS_LDFLAGS 1155
#define _CS_POSIX_V8_ILP32_OFF32_CFLAGS 1160
#define _CS_POSIX_V8_ILP32_OFF32_LDFLAGS 1161
#define _CS_POSIX_V8_ILP32_OFF32_LIBS 1162
#define _CS_POSIX_V8_ILP32_OFFBIG_CFLAGS 1164
#define _CS_POSIX_V8_ILP32_OFFBIG_LDFLAGS 1165
#define _CS_POSIX_V8_ILP32_OFFBIG_LIBS 1166
#define _CS_POSIX_V8_LP64_OFF64_CFLAGS 1168
#define _CS_POSIX_V8_LP64_OFF64_LDFLAGS 1169
#define _CS_POSIX_V8_LP64_OFF64_LIBS 1170
#define _CS_POSIX_V8_LPBIG_OFFBIG_CFLAGS 1172
#define _CS_POSIX_V8_LPBIG_OFFBIG_LDFLAGS 1173
#define _CS_POSIX_V8_LPBIG_OFFBIG_LIBS 1174

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
