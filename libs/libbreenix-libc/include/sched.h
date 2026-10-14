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
#ifdef __BREENIX_EXT
/*
 * GNU CPU sets: CPU_SETSIZE bits in unsigned longs, 128 bytes as on Linux. The _S forms
 * take the set's size in bytes, for sets from CPU_ALLOC. A processor number at or past
 * the set's size is ignored by CPU_SET_S and CPU_CLR_S and reads as clear. Each macro
 * evaluates its arguments once.
 */
#define CPU_SETSIZE 1024
#define __BREENIX_CPU_BITS (8 * sizeof(unsigned long))
typedef struct { unsigned long __bits[CPU_SETSIZE / (8 * sizeof(unsigned long))]; } cpu_set_t;

static __inline__ unsigned long *__breenix_cpu_word(size_t __cpu, size_t __size, cpu_set_t *__set)
{
    return __cpu / 8 < __size ? (unsigned long *)__set + __cpu / __BREENIX_CPU_BITS : 0;
}
static __inline__ void __breenix_cpu_zero(size_t __size, cpu_set_t *__set)
{
    __builtin_memset(__set, 0, __size);
}
static __inline__ void __breenix_cpu_set(size_t __cpu, size_t __size, cpu_set_t *__set)
{
    unsigned long *__word = __breenix_cpu_word(__cpu, __size, __set);
    if (__word)
        *__word |= 1UL << (__cpu % __BREENIX_CPU_BITS);
}
static __inline__ void __breenix_cpu_clr(size_t __cpu, size_t __size, cpu_set_t *__set)
{
    unsigned long *__word = __breenix_cpu_word(__cpu, __size, __set);
    if (__word)
        *__word &= ~(1UL << (__cpu % __BREENIX_CPU_BITS));
}
static __inline__ int __breenix_cpu_isset(size_t __cpu, size_t __size, const cpu_set_t *__set)
{
    const unsigned long *__word = __breenix_cpu_word(__cpu, __size, (cpu_set_t *)__set);
    return __word ? (int)((*__word >> (__cpu % __BREENIX_CPU_BITS)) & 1) : 0;
}
static __inline__ int __breenix_cpu_count(size_t __size, const cpu_set_t *__set)
{
    const unsigned long *__bits = (const unsigned long *)__set;
    size_t __i;
    int __n = 0;
    for (__i = 0; __i < __size / sizeof(unsigned long); __i++)
        __n += __builtin_popcountl(__bits[__i]);
    return __n;
}
static __inline__ int __breenix_cpu_equal(size_t __size, const cpu_set_t *__a, const cpu_set_t *__b)
{
    return __builtin_memcmp(__a, __b, __size) == 0;
}
/* __op: 0 and, 1 or, 2 exclusive or. */
static __inline__ void __breenix_cpu_op(size_t __size, cpu_set_t *__dest, const cpu_set_t *__a,
                                        const cpu_set_t *__b, int __op)
{
    unsigned long *__d = (unsigned long *)__dest;
    const unsigned long *__x = (const unsigned long *)__a;
    const unsigned long *__y = (const unsigned long *)__b;
    size_t __i;
    for (__i = 0; __i < __size / sizeof(unsigned long); __i++)
        __d[__i] = __op == 0 ? __x[__i] & __y[__i] : __op == 1 ? __x[__i] | __y[__i] : __x[__i] ^ __y[__i];
}

#define CPU_ALLOC_SIZE(count) \
    ((((size_t)(count) + __BREENIX_CPU_BITS - 1) / __BREENIX_CPU_BITS) * sizeof(unsigned long))
#define CPU_ALLOC(count) ((cpu_set_t *)__builtin_malloc(CPU_ALLOC_SIZE(count)))
#define CPU_FREE(set) __builtin_free(set)

#define CPU_ZERO_S(size, set) __breenix_cpu_zero((size), (set))
#define CPU_SET_S(cpu, size, set) __breenix_cpu_set((size_t)(cpu), (size), (set))
#define CPU_CLR_S(cpu, size, set) __breenix_cpu_clr((size_t)(cpu), (size), (set))
#define CPU_ISSET_S(cpu, size, set) __breenix_cpu_isset((size_t)(cpu), (size), (set))
#define CPU_COUNT_S(size, set) __breenix_cpu_count((size), (set))
#define CPU_EQUAL_S(size, a, b) __breenix_cpu_equal((size), (a), (b))
#define CPU_AND_S(size, dest, a, b) __breenix_cpu_op((size), (dest), (a), (b), 0)
#define CPU_OR_S(size, dest, a, b) __breenix_cpu_op((size), (dest), (a), (b), 1)
#define CPU_XOR_S(size, dest, a, b) __breenix_cpu_op((size), (dest), (a), (b), 2)

#define CPU_ZERO(set) CPU_ZERO_S(sizeof(cpu_set_t), set)
#define CPU_SET(cpu, set) CPU_SET_S(cpu, sizeof(cpu_set_t), set)
#define CPU_CLR(cpu, set) CPU_CLR_S(cpu, sizeof(cpu_set_t), set)
#define CPU_ISSET(cpu, set) CPU_ISSET_S(cpu, sizeof(cpu_set_t), set)
#define CPU_COUNT(set) CPU_COUNT_S(sizeof(cpu_set_t), set)
#define CPU_EQUAL(a, b) CPU_EQUAL_S(sizeof(cpu_set_t), a, b)
#define CPU_AND(dest, a, b) CPU_AND_S(sizeof(cpu_set_t), dest, a, b)
#define CPU_OR(dest, a, b) CPU_OR_S(sizeof(cpu_set_t), dest, a, b)
#define CPU_XOR(dest, a, b) CPU_XOR_S(sizeof(cpu_set_t), dest, a, b)

#ifdef __cplusplus
extern "C" {
#endif
int sched_getaffinity(pid_t, size_t, cpu_set_t *);
int sched_setaffinity(pid_t, size_t, const cpu_set_t *);
#ifdef __cplusplus
}
#endif
#endif /* __BREENIX_EXT */
#endif /* __BREENIX_POSIX */
#endif
