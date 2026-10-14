#ifndef __BREENIX_SCHED_H
#define __BREENIX_SCHED_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <sys/types.h>
#include <stddef.h>
#include <__breenix/timespec.h>
struct sched_param { int sched_priority; };
#define SCHED_OTHER 0
#define SCHED_FIFO 1
#define SCHED_RR 2
#ifdef __cplusplus
extern "C" {
#endif
/* Breenix CPU masks use 64-bit words; the public set reserves 1024 CPUs. */
typedef struct { unsigned long __bits[16]; } cpu_set_t;
int sched_getaffinity(pid_t, size_t, cpu_set_t *);
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
#ifdef __BREENIX_EXT
/* GNU CPU sets: CPU_SETSIZE bits in unsigned longs, 128 bytes as on Linux. */
#define CPU_SETSIZE 1024
#define __BREENIX_CPU_BITS (8 * sizeof(unsigned long))
typedef struct { unsigned long __bits[CPU_SETSIZE / (8 * sizeof(unsigned long))]; } cpu_set_t;
#define CPU_ZERO(set) __builtin_memset((set), 0, sizeof(cpu_set_t))
#define CPU_SET(cpu, set) \
    ((void)((set)->__bits[(cpu) / __BREENIX_CPU_BITS] |= 1UL << ((cpu) % __BREENIX_CPU_BITS)))
#define CPU_CLR(cpu, set) \
    ((void)((set)->__bits[(cpu) / __BREENIX_CPU_BITS] &= ~(1UL << ((cpu) % __BREENIX_CPU_BITS))))
#define CPU_ISSET(cpu, set) \
    (((set)->__bits[(cpu) / __BREENIX_CPU_BITS] >> ((cpu) % __BREENIX_CPU_BITS)) & 1)
static inline int __breenix_cpu_count(const cpu_set_t *__set)
{
    int __n = 0;
    for (unsigned __i = 0; __i < sizeof(__set->__bits) / sizeof(__set->__bits[0]); __i++)
        __n += __builtin_popcountl(__set->__bits[__i]);
    return __n;
}
#define CPU_COUNT(set) __breenix_cpu_count(set)
#ifdef __cplusplus
extern "C" {
#endif
int sched_getaffinity(pid_t, size_t, cpu_set_t *);
#ifdef __cplusplus
}
#endif
#endif /* __BREENIX_EXT */
#endif /* __BREENIX_POSIX */
#endif
