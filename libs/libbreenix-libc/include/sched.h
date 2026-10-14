#ifndef __BREENIX_SCHED_H
#define __BREENIX_SCHED_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <sys/types.h>
#include <__breenix/timespec.h>
struct sched_param { int sched_priority; };
#define SCHED_OTHER 0
#define SCHED_FIFO 1
#define SCHED_RR 2
#ifdef __cplusplus
extern "C" {
#endif
int sched_yield(void);
int sched_setscheduler(pid_t, int, const struct sched_param *);
int sched_getscheduler(pid_t);
int sched_setparam(pid_t, const struct sched_param *);
int sched_getparam(pid_t, struct sched_param *);
int sched_get_priority_min(int);
int sched_get_priority_max(int);
int sched_rr_get_interval(pid_t, struct timespec *);
#ifdef __cplusplus
}
#endif
#endif /* __BREENIX_POSIX */
#endif
