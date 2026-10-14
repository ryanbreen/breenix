#ifndef __BREENIX_PTHREAD_H
#define __BREENIX_PTHREAD_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <stddef.h>
#include <__breenix/types.h>
/* POSIX.1-2024: <pthread.h> makes the symbols of <sched.h> and <time.h> visible. */
#include <sched.h>
#include <time.h>
typedef __breenix_pthread_t pthread_t;
typedef __breenix_pthread_key_t pthread_key_t;
typedef __breenix_pthread_once_t pthread_once_t;
typedef __breenix_pthread_spinlock_t pthread_spinlock_t;
typedef __breenix_pthread_attr_t pthread_attr_t;
typedef __breenix_pthread_mutex_t pthread_mutex_t;
typedef __breenix_pthread_mutexattr_t pthread_mutexattr_t;
typedef __breenix_pthread_cond_t pthread_cond_t;
typedef __breenix_pthread_condattr_t pthread_condattr_t;
typedef __breenix_pthread_rwlock_t pthread_rwlock_t;
typedef __breenix_pthread_rwlockattr_t pthread_rwlockattr_t;
typedef __breenix_pthread_barrier_t pthread_barrier_t;
typedef __breenix_pthread_barrierattr_t pthread_barrierattr_t;
#define PTHREAD_CREATE_JOINABLE 0
#define PTHREAD_CREATE_DETACHED 1
#define PTHREAD_MUTEX_NORMAL 0
#define PTHREAD_MUTEX_RECURSIVE 1
#define PTHREAD_MUTEX_ERRORCHECK 2
#define PTHREAD_MUTEX_DEFAULT PTHREAD_MUTEX_NORMAL
#define PTHREAD_MUTEX_STALLED 0
#define PTHREAD_MUTEX_ROBUST 1
#define PTHREAD_PRIO_NONE 0
#define PTHREAD_PRIO_INHERIT 1
#define PTHREAD_PRIO_PROTECT 2
#define PTHREAD_PROCESS_PRIVATE 0
#define PTHREAD_PROCESS_SHARED 1
#define PTHREAD_INHERIT_SCHED 0
#define PTHREAD_EXPLICIT_SCHED 1
#define PTHREAD_SCOPE_SYSTEM 0
#define PTHREAD_SCOPE_PROCESS 1
/* Four pages: the thread's ARM64 signal frame (ucontext_t of 4560 bytes plus
 * siginfo, the kernel's MINSIGSTKSZ of 5120) and the cancellation and
 * cleanup path's frames above it; equal to libs/libc's PTHREAD_STACK_MIN. */
#define PTHREAD_STACK_MIN 16384
#define PTHREAD_DESTRUCTOR_ITERATIONS 4
#define PTHREAD_BARRIER_SERIAL_THREAD (-1)
#define PTHREAD_CANCEL_ENABLE 0
#define PTHREAD_CANCEL_DISABLE 1
#define PTHREAD_CANCEL_DEFERRED 0
#define PTHREAD_CANCEL_ASYNCHRONOUS 1
#define PTHREAD_CANCELED ((void *)-1)
#define PTHREAD_ONCE_INIT 0
/* pthread_t is the thread descriptor's address, never 0 for a thread. */
#define PTHREAD_NULL ((pthread_t)0)
#define PTHREAD_MUTEX_INITIALIZER {{0}}
#define PTHREAD_COND_INITIALIZER {{0}}
#define PTHREAD_RWLOCK_INITIALIZER {{0}}
#ifdef __cplusplus
extern "C" {
#endif
int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *);
int pthread_join(pthread_t, void **);
int pthread_detach(pthread_t);
__BREENIX_NORETURN void pthread_exit(void *);
pthread_t pthread_self(void);
int pthread_equal(pthread_t, pthread_t);
int pthread_once(pthread_once_t *, void (*)(void));
int pthread_atfork(void (*)(void), void (*)(void), void (*)(void));
int pthread_key_create(pthread_key_t *, void (*)(void *));
int pthread_key_delete(pthread_key_t);
int pthread_setspecific(pthread_key_t, const void *);
void *pthread_getspecific(pthread_key_t);
int pthread_attr_init(pthread_attr_t *);
int pthread_attr_destroy(pthread_attr_t *);
int pthread_attr_setstack(pthread_attr_t *, void *, size_t);
int pthread_attr_getstack(const pthread_attr_t *, void **, size_t *);
int pthread_attr_setdetachstate(pthread_attr_t *, int);
int pthread_attr_getdetachstate(const pthread_attr_t *, int *);
int pthread_attr_setstacksize(pthread_attr_t *, size_t);
int pthread_attr_getstacksize(const pthread_attr_t *, size_t *);
int pthread_attr_setguardsize(pthread_attr_t *, size_t);
int pthread_attr_getguardsize(const pthread_attr_t *, size_t *);
int pthread_attr_setinheritsched(pthread_attr_t *, int);
int pthread_attr_getinheritsched(const pthread_attr_t *, int *);
int pthread_attr_setschedpolicy(pthread_attr_t *, int);
int pthread_attr_getschedpolicy(const pthread_attr_t *, int *);
int pthread_attr_setscope(pthread_attr_t *, int);
int pthread_attr_getscope(const pthread_attr_t *, int *);
int pthread_attr_setschedparam(pthread_attr_t *, const struct sched_param *);
int pthread_attr_getschedparam(const pthread_attr_t *, struct sched_param *);
int pthread_setschedparam(pthread_t, int, const struct sched_param *);
int pthread_getschedparam(pthread_t, int *, struct sched_param *);
int pthread_setschedprio(pthread_t, int);
int pthread_mutex_init(pthread_mutex_t *, const pthread_mutexattr_t *);
int pthread_mutex_destroy(pthread_mutex_t *);
int pthread_mutex_lock(pthread_mutex_t *);
int pthread_mutex_trylock(pthread_mutex_t *);
int pthread_mutex_timedlock(pthread_mutex_t *, const struct timespec *);
int pthread_mutex_clocklock(pthread_mutex_t *, clockid_t, const struct timespec *);
int pthread_mutex_unlock(pthread_mutex_t *);
int pthread_mutex_consistent(pthread_mutex_t *);
int pthread_mutex_getprioceiling(const pthread_mutex_t *, int *);
int pthread_mutex_setprioceiling(pthread_mutex_t *, int, int *);
int pthread_mutexattr_init(pthread_mutexattr_t *);
int pthread_mutexattr_destroy(pthread_mutexattr_t *);
int pthread_mutexattr_settype(pthread_mutexattr_t *, int);
int pthread_mutexattr_gettype(const pthread_mutexattr_t *, int *);
int pthread_mutexattr_setrobust(pthread_mutexattr_t *, int);
int pthread_mutexattr_getrobust(const pthread_mutexattr_t *, int *);
int pthread_mutexattr_setprotocol(pthread_mutexattr_t *, int);
int pthread_mutexattr_getprotocol(const pthread_mutexattr_t *, int *);
int pthread_mutexattr_setprioceiling(pthread_mutexattr_t *, int);
int pthread_mutexattr_getprioceiling(const pthread_mutexattr_t *, int *);
int pthread_mutexattr_setpshared(pthread_mutexattr_t *, int);
int pthread_mutexattr_getpshared(const pthread_mutexattr_t *, int *);
int pthread_condattr_init(pthread_condattr_t *);
int pthread_condattr_destroy(pthread_condattr_t *);
int pthread_condattr_setclock(pthread_condattr_t *, int);
int pthread_condattr_getclock(const pthread_condattr_t *, int *);
int pthread_condattr_setpshared(pthread_condattr_t *, int);
int pthread_condattr_getpshared(const pthread_condattr_t *, int *);
int pthread_rwlockattr_init(pthread_rwlockattr_t *);
int pthread_rwlockattr_destroy(pthread_rwlockattr_t *);
int pthread_rwlockattr_setpshared(pthread_rwlockattr_t *, int);
int pthread_rwlockattr_getpshared(const pthread_rwlockattr_t *, int *);
int pthread_barrierattr_init(pthread_barrierattr_t *);
int pthread_barrierattr_destroy(pthread_barrierattr_t *);
int pthread_barrierattr_setpshared(pthread_barrierattr_t *, int);
int pthread_barrierattr_getpshared(const pthread_barrierattr_t *, int *);
int pthread_cond_init(pthread_cond_t *, const pthread_condattr_t *);
int pthread_cond_destroy(pthread_cond_t *);
int pthread_cond_signal(pthread_cond_t *);
int pthread_cond_broadcast(pthread_cond_t *);
int pthread_cond_wait(pthread_cond_t *, pthread_mutex_t *);
int pthread_cond_timedwait(pthread_cond_t *, pthread_mutex_t *, const struct timespec *);
int pthread_cond_clockwait(pthread_cond_t *, pthread_mutex_t *, int, const struct timespec *);
int pthread_rwlock_init(pthread_rwlock_t *, const pthread_rwlockattr_t *);
int pthread_rwlock_destroy(pthread_rwlock_t *);
int pthread_rwlock_rdlock(pthread_rwlock_t *);
int pthread_rwlock_wrlock(pthread_rwlock_t *);
int pthread_rwlock_tryrdlock(pthread_rwlock_t *);
int pthread_rwlock_trywrlock(pthread_rwlock_t *);
int pthread_rwlock_timedrdlock(pthread_rwlock_t *, const struct timespec *);
int pthread_rwlock_timedwrlock(pthread_rwlock_t *, const struct timespec *);
int pthread_rwlock_clockrdlock(pthread_rwlock_t *, clockid_t, const struct timespec *);
int pthread_rwlock_clockwrlock(pthread_rwlock_t *, clockid_t, const struct timespec *);
int pthread_rwlock_unlock(pthread_rwlock_t *);
int pthread_barrier_init(pthread_barrier_t *, const pthread_barrierattr_t *, unsigned int);
int pthread_barrier_wait(pthread_barrier_t *);
int pthread_barrier_destroy(pthread_barrier_t *);
int pthread_spin_init(pthread_spinlock_t *, int);
int pthread_spin_destroy(pthread_spinlock_t *);
int pthread_spin_lock(pthread_spinlock_t *);
int pthread_spin_trylock(pthread_spinlock_t *);
int pthread_spin_unlock(pthread_spinlock_t *);
int pthread_cancel(pthread_t);
int pthread_setcancelstate(int, int *);
int pthread_setcanceltype(int, int *);
void pthread_testcancel(void);
struct __breenix_cleanup {
    struct __breenix_cleanup *previous;
    void (*routine)(void *);
    void *arg;
};
void __breenix_cleanup_push(struct __breenix_cleanup *, void (*)(void *), void *);
void __breenix_cleanup_pop(struct __breenix_cleanup *, int);
/* POSIX permits these paired operations to introduce a lexical block. */
#define pthread_cleanup_push(routine, arg) do { \
    struct __breenix_cleanup __breenix_cleanup_frame; \
    __breenix_cleanup_push(&__breenix_cleanup_frame, (routine), (arg));
#define pthread_cleanup_pop(execute) \
    __breenix_cleanup_pop(&__breenix_cleanup_frame, (execute)); } while (0)
#if __BREENIX_EXT
int pthread_getattr_np(pthread_t, pthread_attr_t *);
int pthread_setname_np(pthread_t, const char *);
#endif
#ifdef __cplusplus
}
#endif
#endif /* __BREENIX_POSIX */
#endif
