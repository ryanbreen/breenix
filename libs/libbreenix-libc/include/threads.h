/* <threads.h>: ISO C17 7.26 threads. Each object is its POSIX counterpart (src/threads.rs):
 * thrd_t is pthread_t, mtx_t pthread_mutex_t, cnd_t pthread_cond_t, tss_t pthread_key_t
 * and once_flag pthread_once_t. */
#ifndef __BREENIX_THREADS_H
#define __BREENIX_THREADS_H
#include <__breenix/features.h>
#include <__breenix/types.h>
#include <time.h>

#ifndef __cplusplus
#define thread_local _Thread_local
#endif
#define ONCE_FLAG_INIT 0
#define TSS_DTOR_ITERATIONS 4

typedef __breenix_pthread_t thrd_t;
typedef __breenix_pthread_mutex_t mtx_t;
typedef __breenix_pthread_cond_t cnd_t;
typedef __breenix_pthread_key_t tss_t;
typedef __breenix_pthread_once_t once_flag;
typedef int (*thrd_start_t)(void *);
typedef void (*tss_dtor_t)(void *);

enum {
    thrd_success = 0,
    thrd_busy = 1,
    thrd_error = 2,
    thrd_nomem = 3,
    thrd_timedout = 4
};

enum {
    mtx_plain = 0,
    mtx_recursive = 1,
    mtx_timed = 2
};

__BREENIX_BEGIN_DECLS
void call_once(once_flag *, void (*)(void));
int cnd_broadcast(cnd_t *);
void cnd_destroy(cnd_t *);
int cnd_init(cnd_t *);
int cnd_signal(cnd_t *);
int cnd_timedwait(cnd_t *__BREENIX_RESTRICT, mtx_t *__BREENIX_RESTRICT, const struct timespec *__BREENIX_RESTRICT);
int cnd_wait(cnd_t *, mtx_t *);
void mtx_destroy(mtx_t *);
int mtx_init(mtx_t *, int);
int mtx_lock(mtx_t *);
int mtx_timedlock(mtx_t *__BREENIX_RESTRICT, const struct timespec *__BREENIX_RESTRICT);
int mtx_trylock(mtx_t *);
int mtx_unlock(mtx_t *);
int thrd_create(thrd_t *, thrd_start_t, void *);
thrd_t thrd_current(void);
int thrd_detach(thrd_t);
int thrd_equal(thrd_t, thrd_t);
__BREENIX_NORETURN void thrd_exit(int);
int thrd_join(thrd_t, int *);
int thrd_sleep(const struct timespec *, struct timespec *);
void thrd_yield(void);
int tss_create(tss_t *, tss_dtor_t);
void tss_delete(tss_t);
void *tss_get(tss_t);
int tss_set(tss_t, void *);
__BREENIX_END_DECLS
#endif
