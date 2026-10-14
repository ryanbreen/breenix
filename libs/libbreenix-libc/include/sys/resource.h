/* POSIX.1-2024 resource control. Limit indices match kernel/src/process/limits.rs; selectors match kernel/src/syscall/handlers.rs. */
#ifndef _SYS_RESOURCE_H
#define _SYS_RESOURCE_H

#include <__breenix/types.h>
#include <__breenix/timeval.h>

typedef __breenix_rlim_t rlim_t;
typedef __breenix_id_t id_t;

#define PRIO_PROCESS 0
#define PRIO_PGRP 1
#define PRIO_USER 2

#define RLIM_INFINITY (~0ul)
#define RLIM_SAVED_MAX RLIM_INFINITY
#define RLIM_SAVED_CUR RLIM_INFINITY

#define RUSAGE_SELF 0
#define RUSAGE_CHILDREN (-1)
#define RUSAGE_THREAD 1

#define RLIMIT_CPU 0
#define RLIMIT_FSIZE 1
#define RLIMIT_DATA 2
#define RLIMIT_STACK 3
#define RLIMIT_CORE 4
#define RLIMIT_RSS 5
#define RLIMIT_NPROC 6
#define RLIMIT_NOFILE 7
#define RLIMIT_MEMLOCK 8
#define RLIMIT_AS 9
#define RLIMIT_LOCKS 10
#define RLIMIT_SIGPENDING 11
#define RLIMIT_MSGQUEUE 12
#define RLIMIT_NICE 13
#define RLIMIT_RTPRIO 14
#define RLIMIT_RTTIME 15
#define RLIMIT_NLIMITS 16
#define RLIM_NLIMITS RLIMIT_NLIMITS

struct rlimit {
    rlim_t rlim_cur;
    rlim_t rlim_max;
};

struct rusage {
    struct timeval ru_utime;
    struct timeval ru_stime;
    long ru_maxrss;
    long ru_ixrss;
    long ru_idrss;
    long ru_isrss;
    long ru_minflt;
    long ru_majflt;
    long ru_nswap;
    long ru_inblock;
    long ru_oublock;
    long ru_msgsnd;
    long ru_msgrcv;
    long ru_nsignals;
    long ru_nvcsw;
    long ru_nivcsw;
};

__BREENIX_BEGIN_DECLS

int getpriority(int, id_t);
int getrlimit(int, struct rlimit *);
int getrusage(int, struct rusage *);
int setpriority(int, id_t, int);
int setrlimit(int, const struct rlimit *);

#ifdef __BREENIX_EXT
int prlimit(__breenix_pid_t, int, const struct rlimit *, struct rlimit *);
#define getrlimit64 getrlimit
#define setrlimit64 setrlimit
#define rlimit64 rlimit
#endif

__BREENIX_END_DECLS

#endif
