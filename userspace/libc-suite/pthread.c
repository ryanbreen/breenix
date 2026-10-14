/*
 * libc suite, pthread category: <pthread.h> as POSIX.1-2024 specifies it. Where POSIX
 * lets an implementation detect a misuse ("may fail"), the case title says it measures
 * Linux's policy. Every wait is bounded: timeouts are 50 ms and a thread that never
 * arrives fails the case after 2 seconds, inside the suite's 10-second case limit.
 */
#define _POSIX_C_SOURCE 202405L
/* syscall(), to queue the C library's reserved cancel signal directly. */
#define _DEFAULT_SOURCE
#include "case.h"

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static long long now_ns(clockid_t clock) {
    struct timespec ts;
    clock_gettime(clock, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static struct timespec deadline(clockid_t clock, long ms) {
    long long t = now_ns(clock) + (long long)ms * 1000000LL;
    struct timespec ts = { (time_t)(t / 1000000000LL), (long)(t % 1000000000LL) };
    return ts;
}

static long long ts_ns(const struct timespec *ts) {
    return (long long)ts->tv_sec * 1000000000LL + ts->tv_nsec;
}

static void pause_ms(long ms) {
    struct timespec ts = { 0, ms * 1000000L };
    nanosleep(&ts, NULL);
}

/* Wait up to 2 seconds for *flag to reach want. */
static int await(atomic_int *flag, int want) {
    long long end = now_ns(CLOCK_MONOTONIC) + 2000000000LL;
    while (atomic_load(flag) < want) {
        if (now_ns(CLOCK_MONOTONIC) > end)
            return 0;
        pause_ms(1);
    }
    return 1;
}

static void *return_42(void *arg) {
    (void)arg;
    return (void *)(intptr_t)42;
}

static const char *create_join(void) {
    pthread_t t;
    void *ret = NULL;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, return_42, NULL), 0);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_INT("value the thread returned", (intptr_t)ret, 42);
    return PASS;
}

static const char *join_self(void) {
    EXPECT_INT("pthread_join(pthread_self())", pthread_join(pthread_self(), NULL), EDEADLK);
    return PASS;
}

static atomic_int ran;

static void *set_ran(void *arg) {
    (void)arg;
    atomic_fetch_add(&ran, 1);
    return NULL;
}

static const char *detach_runs(void) {
    pthread_t t;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, set_ran, NULL), 0);
    EXPECT_INT("pthread_detach", pthread_detach(t), 0);
    CHECK(await(&ran, 1), "the detached thread did not run within 2 seconds");
    return PASS;
}

static const char *attr_detached(void) {
    pthread_attr_t a;
    pthread_t t;
    int state = -1;
    EXPECT_INT("pthread_attr_init", pthread_attr_init(&a), 0);
    EXPECT_INT("pthread_attr_getdetachstate", pthread_attr_getdetachstate(&a, &state), 0);
    EXPECT_INT("default detach state", state, PTHREAD_CREATE_JOINABLE);
    EXPECT_INT("pthread_attr_setdetachstate(99)", pthread_attr_setdetachstate(&a, 99), EINVAL);
    EXPECT_INT("pthread_attr_setdetachstate(DETACHED)", pthread_attr_setdetachstate(&a, PTHREAD_CREATE_DETACHED), 0);
    EXPECT_INT("pthread_create detached", pthread_create(&t, &a, set_ran, NULL), 0);
    CHECK(await(&ran, 1), "the thread created detached did not run within 2 seconds");
    pthread_attr_destroy(&a);
    return PASS;
}

static void exit_deep(int depth) {
    if (depth == 0)
        pthread_exit((void *)(intptr_t)7);
    exit_deep(depth - 1);
}

static void *calls_exit(void *arg) {
    (void)arg;
    exit_deep(3);
    return NULL;
}

static const char *exit_value(void) {
    pthread_t t;
    void *ret = NULL;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, calls_exit, NULL), 0);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_INT("value given to pthread_exit", (intptr_t)ret, 7);
    return PASS;
}

static pthread_t seen_self;

static void *record_self(void *arg) {
    (void)arg;
    seen_self = pthread_self();
    return NULL;
}

static const char *self_equal(void) {
    pthread_t t;
    CHECK(pthread_equal(pthread_self(), pthread_self()), "pthread_equal(self, self) is false");
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, record_self, NULL), 0);
    EXPECT_INT("pthread_join", pthread_join(t, NULL), 0);
    CHECK(pthread_equal(seen_self, t), "pthread_self in the thread differs from the ID pthread_create gave");
    CHECK(!pthread_equal(t, pthread_self()), "a new thread's ID equals the creator's");
    return PASS;
}

static const char *many_threads(void) {
    pthread_t t[16];
    for (int i = 0; i < 16; i++)
        EXPECT_INT("pthread_create", pthread_create(&t[i], NULL, set_ran, NULL), 0);
    for (int i = 0; i < 16; i++)
        EXPECT_INT("pthread_join", pthread_join(t[i], NULL), 0);
    EXPECT_INT("threads that ran", atomic_load(&ran), 16);
    return PASS;
}

static pthread_mutex_t shared = PTHREAD_MUTEX_INITIALIZER;
static atomic_int result;

static void *trylock_shared(void *arg) {
    (void)arg;
    atomic_store(&result, pthread_mutex_trylock(&shared));
    return NULL;
}

static void *unlock_shared(void *arg) {
    (void)arg;
    atomic_store(&result, pthread_mutex_unlock(&shared));
    return NULL;
}

static const char *run_on_thread(void *(*fn)(void *), int *out) {
    pthread_t t;
    if (pthread_create(&t, NULL, fn, NULL) != 0)
        return fail_text("pthread_create failed");
    if (pthread_join(t, NULL) != 0)
        return fail_text("pthread_join failed");
    *out = atomic_load(&result);
    return PASS;
}

#define ON_THREAD(fn, out) do { const char *m_ = run_on_thread((fn), &(out)); if (m_) return m_; } while (0)

static const char *mutex_trylock(void) {
    int r;
    EXPECT_INT("lock", pthread_mutex_lock(&shared), 0);
    ON_THREAD(trylock_shared, r);
    EXPECT_INT("pthread_mutex_trylock from another thread", r, EBUSY);
    EXPECT_INT("pthread_mutex_trylock from the owner", pthread_mutex_trylock(&shared), EBUSY);
    EXPECT_INT("unlock", pthread_mutex_unlock(&shared), 0);
    ON_THREAD(trylock_shared, r);
    EXPECT_INT("pthread_mutex_trylock of an unlocked mutex", r, 0);
    return PASS;
}

static const char *init_typed(int type) {
    pthread_mutexattr_t a;
    if (pthread_mutexattr_init(&a) || pthread_mutexattr_settype(&a, type) || pthread_mutex_init(&shared, &a))
        return fail_text("could not make a mutex of the type");
    pthread_mutexattr_destroy(&a);
    return PASS;
}

static const char *mutex_errorcheck(void) {
    int r;
    const char *m = init_typed(PTHREAD_MUTEX_ERRORCHECK);
    if (m)
        return m;
    EXPECT_INT("unlock of an unlocked error-checking mutex", pthread_mutex_unlock(&shared), EPERM);
    EXPECT_INT("lock", pthread_mutex_lock(&shared), 0);
    EXPECT_INT("relock by the owner", pthread_mutex_lock(&shared), EDEADLK);
    ON_THREAD(unlock_shared, r);
    EXPECT_INT("unlock by a thread that is not the owner", r, EPERM);
    EXPECT_INT("unlock by the owner", pthread_mutex_unlock(&shared), 0);
    return PASS;
}

static const char *mutex_recursive(void) {
    int r;
    const char *m = init_typed(PTHREAD_MUTEX_RECURSIVE);
    if (m)
        return m;
    EXPECT_INT("first lock", pthread_mutex_lock(&shared), 0);
    EXPECT_INT("second lock", pthread_mutex_lock(&shared), 0);
    EXPECT_INT("trylock by the owner", pthread_mutex_trylock(&shared), 0);
    EXPECT_INT("first unlock", pthread_mutex_unlock(&shared), 0);
    EXPECT_INT("second unlock", pthread_mutex_unlock(&shared), 0);
    ON_THREAD(trylock_shared, r);
    EXPECT_INT("trylock from another thread while still held once", r, EBUSY);
    EXPECT_INT("last unlock", pthread_mutex_unlock(&shared), 0);
    EXPECT_INT("unlock once too often", pthread_mutex_unlock(&shared), EPERM);
    return PASS;
}

static long counter;

static void *add_many(void *arg) {
    (void)arg;
    for (int i = 0; i < 20000; i++) {
        pthread_mutex_lock(&shared);
        counter++;
        pthread_mutex_unlock(&shared);
    }
    return NULL;
}

static const char *mutex_contention(void) {
    pthread_t t[4];
    for (int i = 0; i < 4; i++)
        EXPECT_INT("pthread_create", pthread_create(&t[i], NULL, add_many, NULL), 0);
    for (int i = 0; i < 4; i++)
        EXPECT_INT("pthread_join", pthread_join(t[i], NULL), 0);
    EXPECT_INT("count after 4 threads added 20000 each under the mutex", counter, 80000);
    return PASS;
}

static atomic_int holding, release;

static void *hold_shared(void *arg) {
    (void)arg;
    pthread_mutex_lock(&shared);
    atomic_store(&holding, 1);
    await(&release, 1);
    pthread_mutex_unlock(&shared);
    return NULL;
}

static const char *mutex_timedlock(void) {
    pthread_t t;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, hold_shared, NULL), 0);
    CHECK(await(&holding, 1), "the holder did not take the mutex within 2 seconds");
    struct timespec until = deadline(CLOCK_REALTIME, 50);
    int r = pthread_mutex_timedlock(&shared, &until);
    long long after = now_ns(CLOCK_REALTIME);
    atomic_store(&release, 1);
    pthread_join(t, NULL);
    EXPECT_INT("pthread_mutex_timedlock of a held mutex", r, ETIMEDOUT);
    CHECK(after >= ts_ns(&until), "pthread_mutex_timedlock returned before its deadline");
    return PASS;
}

static const char *mutexattr_type(void) {
    pthread_mutexattr_t a;
    int type = -1;
    EXPECT_INT("pthread_mutexattr_init", pthread_mutexattr_init(&a), 0);
    EXPECT_INT("pthread_mutexattr_gettype", pthread_mutexattr_gettype(&a, &type), 0);
    EXPECT_INT("default type", type, PTHREAD_MUTEX_DEFAULT);
    EXPECT_INT("pthread_mutexattr_settype(99)", pthread_mutexattr_settype(&a, 99), EINVAL);
    EXPECT_INT("settype(ERRORCHECK)", pthread_mutexattr_settype(&a, PTHREAD_MUTEX_ERRORCHECK), 0);
    pthread_mutexattr_gettype(&a, &type);
    EXPECT_INT("type after settype", type, PTHREAD_MUTEX_ERRORCHECK);
    EXPECT_INT("pthread_mutexattr_destroy", pthread_mutexattr_destroy(&a), 0);
    return PASS;
}

static void *lock_and_exit(void *arg) {
    (void)arg;
    pthread_mutex_lock(&shared);
    return NULL;
}

static const char *mutex_robust(void) {
    pthread_mutexattr_t a;
    pthread_t t;
    EXPECT_INT("pthread_mutexattr_init", pthread_mutexattr_init(&a), 0);
    EXPECT_INT("setrobust(ROBUST)", pthread_mutexattr_setrobust(&a, PTHREAD_MUTEX_ROBUST), 0);
    EXPECT_INT("pthread_mutex_init", pthread_mutex_init(&shared, &a), 0);
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, lock_and_exit, NULL), 0);
    EXPECT_INT("pthread_join", pthread_join(t, NULL), 0);
    EXPECT_INT("lock after the owner exited holding it", pthread_mutex_lock(&shared), EOWNERDEAD);
    EXPECT_INT("pthread_mutex_consistent", pthread_mutex_consistent(&shared), 0);
    EXPECT_INT("unlock", pthread_mutex_unlock(&shared), 0);
    EXPECT_INT("lock after recovery", pthread_mutex_lock(&shared), 0);
    EXPECT_INT("unlock", pthread_mutex_unlock(&shared), 0);
    return PASS;
}

static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
static int ready, waiting, woken;

static void *consume(void *arg) {
    (void)arg;
    pthread_mutex_lock(&shared);
    waiting++;
    while (!ready)
        pthread_cond_wait(&cond, &shared);
    woken++;
    pthread_mutex_unlock(&shared);
    return NULL;
}

static int waiting_now(void) {
    pthread_mutex_lock(&shared);
    int n = waiting;
    pthread_mutex_unlock(&shared);
    return n;
}

static const char *cond_signal(void) {
    pthread_t t;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, consume, NULL), 0);
    long long end = now_ns(CLOCK_MONOTONIC) + 2000000000LL;
    while (waiting_now() < 1 && now_ns(CLOCK_MONOTONIC) < end)
        pause_ms(1);
    pthread_mutex_lock(&shared);
    ready = 1;
    EXPECT_INT("pthread_cond_signal", pthread_cond_signal(&cond), 0);
    pthread_mutex_unlock(&shared);
    EXPECT_INT("pthread_join", pthread_join(t, NULL), 0);
    EXPECT_INT("waiters woken", woken, 1);
    return PASS;
}

static const char *cond_broadcast(void) {
    pthread_t t[4];
    for (int i = 0; i < 4; i++)
        EXPECT_INT("pthread_create", pthread_create(&t[i], NULL, consume, NULL), 0);
    long long end = now_ns(CLOCK_MONOTONIC) + 2000000000LL;
    while (waiting_now() < 4 && now_ns(CLOCK_MONOTONIC) < end)
        pause_ms(1);
    EXPECT_INT("threads waiting before the broadcast", waiting_now(), 4);
    pause_ms(20);
    pthread_mutex_lock(&shared);
    ready = 1;
    EXPECT_INT("pthread_cond_broadcast", pthread_cond_broadcast(&cond), 0);
    pthread_mutex_unlock(&shared);
    for (int i = 0; i < 4; i++)
        EXPECT_INT("pthread_join", pthread_join(t[i], NULL), 0);
    EXPECT_INT("waiters woken by one broadcast", woken, 4);
    return PASS;
}

static const char *cond_timedwait_timeout(void) {
    const char *m = init_typed(PTHREAD_MUTEX_ERRORCHECK);
    if (m)
        return m;
    EXPECT_INT("lock", pthread_mutex_lock(&shared), 0);
    struct timespec until = deadline(CLOCK_REALTIME, 50);
    int r = pthread_cond_timedwait(&cond, &shared, &until);
    long long after = now_ns(CLOCK_REALTIME);
    EXPECT_INT("pthread_cond_timedwait with nobody signalling", r, ETIMEDOUT);
    CHECK(after >= ts_ns(&until), "pthread_cond_timedwait returned before its deadline");
    EXPECT_INT("unlock: the mutex is held again after the timeout", pthread_mutex_unlock(&shared), 0);
    return PASS;
}

static const char *cond_monotonic(void) {
    pthread_condattr_t a;
    pthread_cond_t c;
    clockid_t clock = -1;
    EXPECT_INT("pthread_condattr_init", pthread_condattr_init(&a), 0);
    EXPECT_INT("pthread_condattr_getclock", pthread_condattr_getclock(&a, &clock), 0);
    EXPECT_INT("default clock", clock, CLOCK_REALTIME);
    EXPECT_INT("setclock(CLOCK_MONOTONIC)", pthread_condattr_setclock(&a, CLOCK_MONOTONIC), 0);
    EXPECT_INT("pthread_cond_init", pthread_cond_init(&c, &a), 0);
    pthread_mutex_lock(&shared);
    struct timespec until = deadline(CLOCK_MONOTONIC, 50);
    int r = pthread_cond_timedwait(&c, &shared, &until);
    long long after = now_ns(CLOCK_MONOTONIC);
    pthread_mutex_unlock(&shared);
    EXPECT_INT("pthread_cond_timedwait on CLOCK_MONOTONIC", r, ETIMEDOUT);
    CHECK(after >= ts_ns(&until), "the wait ended before its CLOCK_MONOTONIC deadline");
    CHECK(after - ts_ns(&until) < 1000000000LL, "the wait ended more than a second after its deadline");
    return PASS;
}

static const char *cond_past_deadline(void) {
    struct timespec past = deadline(CLOCK_REALTIME, -1000);
    pthread_mutex_lock(&shared);
    long long before = now_ns(CLOCK_MONOTONIC);
    int r = pthread_cond_timedwait(&cond, &shared, &past);
    long long took = now_ns(CLOCK_MONOTONIC) - before;
    pthread_mutex_unlock(&shared);
    EXPECT_INT("pthread_cond_timedwait with a deadline already past", r, ETIMEDOUT);
    CHECK(took < 500000000LL, "a wait whose deadline had passed took more than 500 ms");
    return PASS;
}

static pthread_rwlock_t rw = PTHREAD_RWLOCK_INITIALIZER;
static atomic_int rd_result, wr_result, timed_result;

static void *try_rw(void *arg) {
    (void)arg;
    int r = pthread_rwlock_tryrdlock(&rw);
    atomic_store(&rd_result, r);
    if (r == 0)
        pthread_rwlock_unlock(&rw);
    r = pthread_rwlock_trywrlock(&rw);
    atomic_store(&wr_result, r);
    if (r == 0)
        pthread_rwlock_unlock(&rw);
    struct timespec until = deadline(CLOCK_REALTIME, 50);
    r = pthread_rwlock_timedrdlock(&rw, &until);
    atomic_store(&timed_result, r);
    if (r == 0)
        pthread_rwlock_unlock(&rw);
    return NULL;
}

static const char *rwlock_readers(void) {
    pthread_t t;
    EXPECT_INT("pthread_rwlock_rdlock", pthread_rwlock_rdlock(&rw), 0);
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, try_rw, NULL), 0);
    pthread_join(t, NULL);
    EXPECT_INT("tryrdlock from another thread while read-locked", atomic_load(&rd_result), 0);
    EXPECT_INT("trywrlock from another thread while read-locked", atomic_load(&wr_result), EBUSY);
    EXPECT_INT("unlock", pthread_rwlock_unlock(&rw), 0);
    return PASS;
}

static const char *rwlock_writer(void) {
    pthread_t t;
    EXPECT_INT("pthread_rwlock_wrlock", pthread_rwlock_wrlock(&rw), 0);
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, try_rw, NULL), 0);
    pthread_join(t, NULL);
    EXPECT_INT("tryrdlock from another thread while write-locked", atomic_load(&rd_result), EBUSY);
    EXPECT_INT("trywrlock from another thread while write-locked", atomic_load(&wr_result), EBUSY);
    EXPECT_INT("timedrdlock from another thread while write-locked", atomic_load(&timed_result), ETIMEDOUT);
    EXPECT_INT("unlock", pthread_rwlock_unlock(&rw), 0);
    return PASS;
}

static const char *rwlock_self_deadlock(void) {
    EXPECT_INT("pthread_rwlock_wrlock", pthread_rwlock_wrlock(&rw), 0);
    EXPECT_INT("wrlock by the thread holding the write lock", pthread_rwlock_wrlock(&rw), EDEADLK);
    EXPECT_INT("rdlock by the thread holding the write lock", pthread_rwlock_rdlock(&rw), EDEADLK);
    EXPECT_INT("unlock", pthread_rwlock_unlock(&rw), 0);
    return PASS;
}

static pthread_once_t once = PTHREAD_ONCE_INIT;
static atomic_int inits, done_init, saw_done;

static void init_once(void) {
    atomic_fetch_add(&inits, 1);
    pause_ms(20);
    atomic_store(&done_init, 1);
}

static void *call_once(void *arg) {
    (void)arg;
    pthread_once(&once, init_once);
    if (atomic_load(&done_init))
        atomic_fetch_add(&saw_done, 1);
    return NULL;
}

static const char *once_only(void) {
    pthread_t t[8];
    for (int i = 0; i < 8; i++)
        EXPECT_INT("pthread_create", pthread_create(&t[i], NULL, call_once, NULL), 0);
    for (int i = 0; i < 8; i++)
        pthread_join(t[i], NULL);
    EXPECT_INT("times the init routine ran", atomic_load(&inits), 1);
    EXPECT_INT("threads that saw it finished when pthread_once returned", atomic_load(&saw_done), 8);
    return PASS;
}

static pthread_key_t key;
static atomic_int destructed_sum, own_value_ok;

static void destructor(void *value) {
    atomic_fetch_add(&destructed_sum, (int)(intptr_t)value);
}

static void *use_key(void *arg) {
    if (pthread_getspecific(key) != NULL)
        return NULL;
    pthread_setspecific(key, arg);
    pause_ms(10);
    if (pthread_getspecific(key) == arg)
        atomic_fetch_add(&own_value_ok, 1);
    return NULL;
}

static const char *keys_destructor(void) {
    pthread_t a, b;
    EXPECT_INT("pthread_key_create", pthread_key_create(&key, destructor), 0);
    EXPECT_INT("pthread_create", pthread_create(&a, NULL, use_key, (void *)(intptr_t)3), 0);
    EXPECT_INT("pthread_create", pthread_create(&b, NULL, use_key, (void *)(intptr_t)5), 0);
    pthread_join(a, NULL);
    pthread_join(b, NULL);
    EXPECT_INT("threads that read back their own value", atomic_load(&own_value_ok), 2);
    EXPECT_INT("sum of the values the destructor received", atomic_load(&destructed_sum), 8);
    EXPECT_PTR("the main thread's value", pthread_getspecific(key), NULL);
    EXPECT_INT("pthread_key_delete", pthread_key_delete(key), 0);
    return PASS;
}

static atomic_int started, past_cancel;

/* Call pthread_testcancel for up to 2 seconds; a thread that is never cancelled
 * returns NULL instead of PTHREAD_CANCELED. */
static void testcancel_for_2s(void) {
    long long end = now_ns(CLOCK_MONOTONIC) + 2000000000LL;
    while (now_ns(CLOCK_MONOTONIC) < end) {
        pthread_testcancel();
        pause_ms(1);
    }
}

static void *spin_testcancel(void *arg) {
    (void)arg;
    atomic_store(&started, 1);
    testcancel_for_2s();
    return NULL;
}

static const char *cancel_testcancel(void) {
    pthread_t t;
    void *ret = NULL;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, spin_testcancel, NULL), 0);
    CHECK(await(&started, 1), "the thread did not start within 2 seconds");
    EXPECT_INT("pthread_cancel", pthread_cancel(t), 0);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_PTR("value of a cancelled thread", ret, PTHREAD_CANCELED);
    return PASS;
}

static int block_pipe[2];
static atomic_int finished;

static void mark_finished(void *arg) {
    (void)arg;
    atomic_store(&finished, 1);
}

static void *block_in_read(void *arg) {
    char c;
    (void)arg;
    pthread_cleanup_push(mark_finished, NULL);
    atomic_store(&started, 1);
    if (read(block_pipe[0], &c, 1) >= 0)
        atomic_store(&past_cancel, 1);
    pthread_cleanup_pop(1);
    return NULL;
}

static const char *cancel_blocking_read(void) {
    pthread_t t;
    void *ret = NULL;
    CHECK(pipe(block_pipe) == 0, "pipe failed");
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, block_in_read, NULL), 0);
    CHECK(await(&started, 1), "the thread did not start within 2 seconds");
    pause_ms(50);
    EXPECT_INT("pthread_cancel", pthread_cancel(t), 0);
    int stuck = !await(&finished, 1);
    if (stuck && write(block_pipe[1], "x", 1) != 1)
        return fail_text("write to the pipe failed");
    int r = pthread_join(t, &ret);
    CHECK(!stuck, "the thread was still blocked in read 2 seconds after pthread_cancel");
    EXPECT_INT("pthread_join", r, 0);
    EXPECT_PTR("value of a thread cancelled in read", ret, PTHREAD_CANCELED);
    EXPECT_INT("read returned after the cancel", atomic_load(&past_cancel), 0);
    return PASS;
}

static atomic_int cancel_sent;

static void *cancel_disabled_thread(void *arg) {
    int old;
    (void)arg;
    pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, &old);
    atomic_store(&started, 1);
    await(&cancel_sent, 1);
    pthread_testcancel();
    atomic_store(&past_cancel, 1);
    pthread_setcancelstate(PTHREAD_CANCEL_ENABLE, &old);
    pthread_testcancel();
    atomic_store(&past_cancel, 2);
    return NULL;
}

static const char *cancel_disabled(void) {
    pthread_t t;
    void *ret = NULL;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, cancel_disabled_thread, NULL), 0);
    CHECK(await(&started, 1), "the thread did not start within 2 seconds");
    EXPECT_INT("pthread_cancel", pthread_cancel(t), 0);
    atomic_store(&cancel_sent, 1);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_PTR("value after the cancel was acted on", ret, PTHREAD_CANCELED);
    EXPECT_INT("progress (1: ran on while disabled, 2: never cancelled)", atomic_load(&past_cancel), 1);
    return PASS;
}

static atomic_int cleaned;

static void cleanup(void *arg) {
    atomic_store(&cleaned, (int)(intptr_t)arg);
}

static void *cancel_with_cleanup(void *arg) {
    (void)arg;
    pthread_cleanup_push(cleanup, (void *)(intptr_t)9);
    atomic_store(&started, 1);
    testcancel_for_2s();
    pthread_cleanup_pop(0);
    return NULL;
}

static const char *cancel_cleanup(void) {
    pthread_t t;
    void *ret = NULL;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, cancel_with_cleanup, NULL), 0);
    CHECK(await(&started, 1), "the thread did not start within 2 seconds");
    pthread_cancel(t);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_PTR("value of the cancelled thread", ret, PTHREAD_CANCELED);
    EXPECT_INT("argument the cleanup handler ran with", atomic_load(&cleaned), 9);
    return PASS;
}

static int count_pipe[2];
static atomic_long bytes_read;

static void *read_one_byte_at_a_time(void *arg) {
    char c;
    (void)arg;
    atomic_store(&started, 1);
    for (;;) {
        if (read(count_pipe[0], &c, 1) == 1)
            atomic_fetch_add(&bytes_read, 1);
    }
    return NULL;
}

/* Fill the pipe whose write end is fd, leaving fd non-blocking; the bytes written. */
static long fill_pipe(int fd) {
    static char block[4096];
    long total = 0;
    if (fcntl(fd, F_SETFL, O_NONBLOCK) != 0)
        return -1;
    for (long n; (n = (long)write(fd, block, sizeof block)) > 0;)
        total += n;
    /* Then byte by byte, so not even a short write fits. */
    while (write(fd, block, 1) == 1)
        total++;
    return total;
}

/* Read what the non-blocking descriptor holds; the byte count. */
static long drain_pipe(int fd) {
    static char block[4096];
    long total = 0, n;
    if (fcntl(fd, F_SETFL, O_NONBLOCK) != 0)
        return -1;
    while ((n = (long)read(fd, block, sizeof block)) > 0)
        total += n;
    return total;
}

/* A thread reading one byte per call spends most of its time in read, so the
 * cancel signal mostly arrives as a read returns. Acting on the request then
 * must not discard the byte that read took (POSIX.1-2024 2.9.5.2: the side
 * effects are those of an EINTR return, which takes no data). */
static const char *cancel_completed_read(void) {
    for (int round = 0; round < 8; round++) {
        pthread_t t;
        void *ret = NULL;
        CHECK(pipe(count_pipe) == 0, "pipe failed");
        long total = fill_pipe(count_pipe[1]);
        CHECK(total > 0, "filling the pipe failed");
        atomic_store(&started, 0);
        atomic_store(&bytes_read, 0);
        EXPECT_INT("pthread_create", pthread_create(&t, NULL, read_one_byte_at_a_time, NULL), 0);
        CHECK(await(&started, 1), "the thread did not start within 2 seconds");
        pause_ms(1 + round);
        EXPECT_INT("pthread_cancel", pthread_cancel(t), 0);
        EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
        EXPECT_PTR("value of the cancelled reader", ret, PTHREAD_CANCELED);
        long left = drain_pipe(count_pipe[0]);
        close(count_pipe[0]);
        close(count_pipe[1]);
        EXPECT_INT("bytes the cancelled thread counted plus bytes left in the pipe",
            atomic_load(&bytes_read) + left, total);
    }
    return PASS;
}

static atomic_int sleep_errno, read_errno;

static void *disabled_waits(void *arg) {
    int old;
    char c;
    struct timespec ts = { 0, 200 * 1000000L };
    (void)arg;
    pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, &old);
    atomic_store(&started, 1);
    atomic_store(&sleep_errno, nanosleep(&ts, NULL) == 0 ? 0 : errno);
    atomic_store(&started, 2);
    long n = (long)read(block_pipe[0], &c, 1);
    atomic_store(&read_errno, n == 1 ? 0 : n < 0 ? errno : -1);
    pthread_setcancelstate(PTHREAD_CANCEL_ENABLE, &old);
    pthread_testcancel();
    return NULL;
}

static const char *cancel_disabled_no_eintr(void) {
    pthread_t t;
    void *ret = NULL;
    CHECK(pipe(block_pipe) == 0, "pipe failed");
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, disabled_waits, NULL), 0);
    CHECK(await(&started, 1), "the thread did not start within 2 seconds");
    pause_ms(50);
    EXPECT_INT("pthread_cancel during nanosleep", pthread_cancel(t), 0);
    CHECK(await(&started, 2), "nanosleep(200 ms) had not returned 2 seconds later");
    pause_ms(50);
    EXPECT_INT("pthread_cancel during read", pthread_cancel(t), 0);
    pause_ms(50);
    CHECK(write(block_pipe[1], "x", 1) == 1, "write to the pipe failed");
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_INT("nanosleep's errno (0: it slept the whole time)", atomic_load(&sleep_errno), 0);
    EXPECT_INT("read's errno (0: it returned the byte written)", atomic_load(&read_errno), 0);
    EXPECT_PTR("value once cancellation was enabled", ret, PTHREAD_CANCELED);
    return PASS;
}

static FILE *full_stream;

static void *write_to_full_pipe(void *arg) {
    (void)arg;
    pthread_cleanup_push(mark_finished, NULL);
    atomic_store(&started, 1);
    fputs("written once the reader drains the pipe\n", full_stream);
    fflush(full_stream);
    pthread_testcancel();
    atomic_store(&past_cancel, 1);
    pthread_cleanup_pop(1);
    return NULL;
}

/* fputs and fflush may be cancellation points, and a thread cancelled in them
 * must not leave the stream's lock, or the list of open streams, held. */
static const char *cancel_stdio_unlocks(void) {
    int fds[2];
    pthread_t t;
    void *ret = NULL;
    CHECK(pipe(fds) == 0, "pipe failed");
    CHECK(fill_pipe(fds[1]) > 0, "filling the pipe failed");
    CHECK(fcntl(fds[1], F_SETFL, 0) == 0, "clearing O_NONBLOCK failed");
    full_stream = fdopen(fds[1], "w");
    CHECK(full_stream != NULL, "fdopen failed");
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, write_to_full_pipe, NULL), 0);
    CHECK(await(&started, 1), "the thread did not start within 2 seconds");
    pause_ms(50);
    EXPECT_INT("pthread_cancel", pthread_cancel(t), 0);
    pause_ms(50);
    long long end = now_ns(CLOCK_MONOTONIC) + 2000000000LL;
    while (!atomic_load(&finished) && now_ns(CLOCK_MONOTONIC) < end) {
        drain_pipe(fds[0]);
        pause_ms(1);
    }
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_PTR("value of the cancelled writer", ret, PTHREAD_CANCELED);
    EXPECT_INT("ftrylockfile on the stream after the cancel", ftrylockfile(full_stream), 0);
    funlockfile(full_stream);
    EXPECT_INT("fclose", fclose(full_stream), 0);
    FILE *f = fopen("cancel-stdio", "w");
    CHECK(f != NULL, "fopen after the cancel failed");
    EXPECT_INT("fclose of a new stream", fclose(f), 0);
    close(fds[0]);
    return PASS;
}

static pthread_mutex_t ticket_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t ticket_cond = PTHREAD_COND_INITIALIZER;
static int tickets;
static atomic_int ticket_waiters, tickets_taken;

static void unlock_tickets(void *arg) {
    (void)arg;
    pthread_mutex_unlock(&ticket_lock);
}

static void *take_ticket(void *arg) {
    (void)arg;
    pthread_mutex_lock(&ticket_lock);
    pthread_cleanup_push(unlock_tickets, NULL);
    atomic_fetch_add(&ticket_waiters, 1);
    while (tickets == 0)
        pthread_cond_wait(&ticket_cond, &ticket_lock);
    tickets--;
    atomic_fetch_add(&tickets_taken, 1);
    pthread_cleanup_pop(1);
    return NULL;
}

/* Two threads wait; one is cancelled just before a single signal. Whichever
 * wakes, the ticket must be taken: a cancelled waiter may not swallow the
 * signal while the other still waits (pthread_cond_wait, POSIX.1-2024). */
static const char *cancel_cond_signal(void) {
    for (int round = 0; round < 20; round++) {
        pthread_t a, b;
        tickets = 0;
        atomic_store(&ticket_waiters, 0);
        atomic_store(&tickets_taken, 0);
        EXPECT_INT("pthread_create", pthread_create(&a, NULL, take_ticket, NULL), 0);
        EXPECT_INT("pthread_create", pthread_create(&b, NULL, take_ticket, NULL), 0);
        CHECK(await(&ticket_waiters, 2), "the waiters did not start within 2 seconds");
        /* Both release the mutex only inside pthread_cond_wait. */
        pthread_mutex_lock(&ticket_lock);
        pthread_mutex_unlock(&ticket_lock);
        pause_ms(2);
        EXPECT_INT("pthread_cancel", pthread_cancel(a), 0);
        pthread_mutex_lock(&ticket_lock);
        tickets = 1;
        pthread_cond_signal(&ticket_cond);
        pthread_mutex_unlock(&ticket_lock);
        int taken = await(&tickets_taken, 1);
        pthread_mutex_lock(&ticket_lock);
        tickets += 2;
        pthread_cond_broadcast(&ticket_cond);
        pthread_mutex_unlock(&ticket_lock);
        pthread_join(a, NULL);
        pthread_join(b, NULL);
        CHECK(taken, "no waiter took the ticket within 2 seconds of pthread_cond_signal");
    }
    return PASS;
}

#if defined(__aarch64__)
#define NR_TGKILL 131
#elif defined(__x86_64__)
#define NR_TGKILL 234
#endif

/* A cancel request can be pending when a thread with cancellation disabled
 * execs: a pthread_cancel that ran between the thread blocking the cancel
 * signal and recording the disabled state queues it behind the mask. Exec
 * resets that signal's action to SIG_DFL, and the new image must still reach
 * main. The child queues the signal itself (the race's outcome), then execs
 * this program to run create-join. */
static const char *cancel_pending_exec(void) {
    int p[2];
    char b[64];
    CHECK(pipe(p) == 0, "pipe failed");
    pid_t pid = fork();
    CHECK(pid >= 0, "fork failed");
    if (pid == 0) {
        close(p[0]);
        if (pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, NULL) != 0)
            _exit(3);
        if (syscall(NR_TGKILL, (long)getpid(), (long)getpid(), 32L) != 0)
            _exit(4);
        char fd[12];
        int n = 0;
        char digits[12];
        for (int v = p[1]; ; v /= 10) {
            digits[n++] = (char)('0' + v % 10);
            if (v < 10)
                break;
        }
        for (int i = 0; i < n; i++)
            fd[i] = digits[n - 1 - i];
        fd[n] = 0;
        char *argv[] = { "/usr/local/test/bin/libc-pthread_test", "create-join", fd, NULL };
        execve(argv[0], argv, libc_case_envp);
        _exit(5);
    }
    close(p[1]);
    long n = 0;
    for (;;) {
        ssize_t r = read(p[0], b + n, sizeof b - 1 - (size_t)n);
        if (r <= 0)
            break;
        n += r;
    }
    b[n] = 0;
    close(p[0]);
    int status = 0;
    EXPECT_INT("waitpid", waitpid(pid, &status, 0), pid);
    if (WIFSIGNALED(status))
        return fail_int("signal that ended the exec'd program (0: none)", WTERMSIG(status), 0);
    EXPECT_INT("child's exit status (3-5: it failed before the exec'd program ran)",
               WEXITSTATUS(status), 0);
    EXPECT_STR("what the exec'd program's create-join reported", b, "PASS\n");
    return PASS;
}

static pthread_barrier_t barrier;
static atomic_int arrived, serial, early;

static void *at_barrier(void *arg) {
    (void)arg;
    atomic_fetch_add(&arrived, 1);
    int r = pthread_barrier_wait(&barrier);
    if (r == PTHREAD_BARRIER_SERIAL_THREAD)
        atomic_fetch_add(&serial, 1);
    else if (r != 0)
        atomic_fetch_add(&early, 100);
    if (atomic_load(&arrived) != 4)
        atomic_fetch_add(&early, 1);
    return NULL;
}

static const char *barrier_wait(void) {
    pthread_t t[4];
    EXPECT_INT("pthread_barrier_init with count 0", pthread_barrier_init(&barrier, NULL, 0), EINVAL);
    EXPECT_INT("pthread_barrier_init", pthread_barrier_init(&barrier, NULL, 4), 0);
    for (int i = 0; i < 4; i++)
        EXPECT_INT("pthread_create", pthread_create(&t[i], NULL, at_barrier, NULL), 0);
    for (int i = 0; i < 4; i++)
        pthread_join(t[i], NULL);
    EXPECT_INT("threads given PTHREAD_BARRIER_SERIAL_THREAD", atomic_load(&serial), 1);
    EXPECT_INT("threads released early or given an error", atomic_load(&early), 0);
    EXPECT_INT("pthread_barrier_destroy", pthread_barrier_destroy(&barrier), 0);
    return PASS;
}

static pthread_spinlock_t spin;

static void *try_spin(void *arg) {
    (void)arg;
    int r = pthread_spin_trylock(&spin);
    atomic_store(&result, r);
    if (r == 0)
        pthread_spin_unlock(&spin);
    return NULL;
}

static const char *spin_lock(void) {
    int r;
    EXPECT_INT("pthread_spin_init", pthread_spin_init(&spin, PTHREAD_PROCESS_PRIVATE), 0);
    EXPECT_INT("pthread_spin_lock", pthread_spin_lock(&spin), 0);
    ON_THREAD(try_spin, r);
    EXPECT_INT("trylock of a held spin lock", r, EBUSY);
    EXPECT_INT("pthread_spin_unlock", pthread_spin_unlock(&spin), 0);
    ON_THREAD(try_spin, r);
    EXPECT_INT("trylock of a free spin lock", r, 0);
    EXPECT_INT("pthread_spin_destroy", pthread_spin_destroy(&spin), 0);
    return PASS;
}

static void *use_stack(void *arg) {
    volatile char big[128 * 1024];
    (void)arg;
    for (size_t i = 0; i < sizeof big; i += 512)
        big[i] = (char)i;
    return (void *)(intptr_t)big[512];
}

static const char *attr_stacksize(void) {
    pthread_attr_t a;
    pthread_t t;
    size_t size = 0;
    void *ret;
    EXPECT_INT("pthread_attr_init", pthread_attr_init(&a), 0);
    EXPECT_INT("setstacksize below PTHREAD_STACK_MIN", pthread_attr_setstacksize(&a, PTHREAD_STACK_MIN - 1), EINVAL);
    EXPECT_INT("setstacksize(256 KiB)", pthread_attr_setstacksize(&a, 256 * 1024), 0);
    EXPECT_INT("getstacksize", pthread_attr_getstacksize(&a, &size), 0);
    EXPECT_UINT("stack size", size, 256 * 1024);
    EXPECT_INT("pthread_create with a 256 KiB stack", pthread_create(&t, &a, use_stack, NULL), 0);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_INT("value from the thread's 128 KiB array", (intptr_t)ret, (char)512);
    return PASS;
}

static void *set_errno(void *arg) {
    (void)arg;
    errno = EDOM;
    pause_ms(10);
    return (void *)(intptr_t)errno;
}

static const char *errno_per_thread(void) {
    pthread_t t;
    void *ret;
    errno = ERANGE;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, set_errno, NULL), 0);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_INT("the thread's errno", (intptr_t)ret, EDOM);
    EXPECT_INT("the main thread's errno", errno, ERANGE);
    return PASS;
}

static void *block_usr1(void *arg) {
    sigset_t s, cur;
    (void)arg;
    sigemptyset(&s);
    sigaddset(&s, SIGUSR1);
    pthread_sigmask(SIG_BLOCK, &s, NULL);
    pthread_sigmask(SIG_BLOCK, NULL, &cur);
    return (void *)(intptr_t)sigismember(&cur, SIGUSR1);
}

static const char *sigmask_per_thread(void) {
    pthread_t t;
    void *ret;
    sigset_t cur;
    EXPECT_INT("pthread_create", pthread_create(&t, NULL, block_usr1, NULL), 0);
    EXPECT_INT("pthread_join", pthread_join(t, &ret), 0);
    EXPECT_INT("SIGUSR1 in the thread's mask", (intptr_t)ret, 1);
    EXPECT_INT("pthread_sigmask query", pthread_sigmask(SIG_BLOCK, NULL, &cur), 0);
    EXPECT_INT("SIGUSR1 in the main thread's mask", sigismember(&cur, SIGUSR1), 0);
    return PASS;
}

const struct libc_case libc_cases[] = {
    { "create-join", create_join },
    { "join-self", join_self },
    { "detach-runs", detach_runs },
    { "attr-detached", attr_detached },
    { "exit-value", exit_value },
    { "self-equal", self_equal },
    { "many-threads", many_threads },
    { "mutex-trylock", mutex_trylock },
    { "mutex-errorcheck", mutex_errorcheck },
    { "mutex-recursive", mutex_recursive },
    { "mutex-contention", mutex_contention },
    { "mutex-timedlock", mutex_timedlock },
    { "mutexattr-type", mutexattr_type },
    { "mutex-robust", mutex_robust },
    { "cond-signal", cond_signal },
    { "cond-broadcast", cond_broadcast },
    { "cond-timedwait-timeout", cond_timedwait_timeout },
    { "cond-monotonic", cond_monotonic },
    { "cond-past-deadline", cond_past_deadline },
    { "rwlock-readers", rwlock_readers },
    { "rwlock-writer", rwlock_writer },
    { "rwlock-self-deadlock", rwlock_self_deadlock },
    { "once-only", once_only },
    { "keys-destructor", keys_destructor },
    { "cancel-testcancel", cancel_testcancel },
    { "cancel-blocking-read", cancel_blocking_read },
    { "cancel-disabled", cancel_disabled },
    { "cancel-cleanup", cancel_cleanup },
    { "cancel-completed-read", cancel_completed_read },
    { "cancel-disabled-no-eintr", cancel_disabled_no_eintr },
    { "cancel-stdio-unlocks", cancel_stdio_unlocks },
    { "cancel-cond-signal", cancel_cond_signal },
    { "cancel-pending-exec", cancel_pending_exec },
    { "barrier-wait", barrier_wait },
    { "spin-lock", spin_lock },
    { "attr-stacksize", attr_stacksize },
    { "errno-per-thread", errno_per_thread },
    { "sigmask-per-thread", sigmask_per_thread },
    { 0, 0 },
};
