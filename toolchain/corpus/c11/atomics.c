/* N1570 5.1.2.4 and 7.17.3, 7.17.7.4. The pthread subset is the same
 * one used by the guest TLS tests. */
#include <stdatomic.h>
#include <stdio.h>
#include "../tls/threads.h"

enum { rounds = 256, workers = 2 };

static int payload;
static atomic_int published;
static atomic_int acknowledged;
static int messages;

static atomic_int start;
static atomic_int finished[workers];
static atomic_int slots[workers];
static int observed[workers];

struct total {
    unsigned long count;
    unsigned long sum;
    unsigned long inverse;
};

static _Atomic(struct total) total;
static atomic_int cas_ready;
static int retries[workers];

static void message_passing(int id)
{
    /* The acknowledgement orders each read before the next payload write. */
    for (int i = 1; i <= rounds; ++i) {
        if (id == 0) {
            while (atomic_load_explicit(&acknowledged, memory_order_acquire) != i - 1) {}
            payload = i * 7;
            atomic_store_explicit(&published, i, memory_order_release);
        } else {
            while (atomic_load_explicit(&published, memory_order_acquire) != i) {}
            if (payload == i * 7)
                ++messages;
            atomic_store_explicit(&acknowledged, i, memory_order_release);
        }
    }
}

static void store_buffering(int id)
{
    for (int i = 1; i <= rounds; ++i) {
        while (atomic_load_explicit(&start, memory_order_acquire) != i) {}
        atomic_store_explicit(&slots[id], 1, memory_order_seq_cst);
        observed[id] = atomic_load_explicit(&slots[1 - id], memory_order_seq_cst);
        atomic_store_explicit(&finished[id], i, memory_order_release);
    }
}

static void aggregate_updates(int id)
{
    struct total expected = atomic_load_explicit(&total, memory_order_acquire);

    /* Both first CAS attempts use the same snapshot, forcing a failed CAS
     * even when the scheduler runs the updates serially. */
    atomic_fetch_add_explicit(&cas_ready, 1, memory_order_acq_rel);
    while (atomic_load_explicit(&cas_ready, memory_order_acquire) != workers) {}

    for (int i = 0; i < rounds; ++i) {
        struct total desired;
        do {
            desired.count = expected.count + 1;
            desired.sum = expected.sum + id + 1;
            desired.inverse = ~desired.count;
            if (atomic_compare_exchange_strong_explicit(&total, &expected, desired,
                                                        memory_order_acq_rel,
                                                        memory_order_acquire))
                break;
            /* With a strong CAS, no worker can lose more often than the
             * other worker advances the count. */
            if (++retries[id] > rounds)
                return;
        } while (1);
        expected = desired;
    }
}

static void *exercise(void *arg)
{
    int id = *(int *)arg;
    message_passing(id);
    store_buffering(id);
    aggregate_updates(id);
    return NULL;
}

int main(void)
{
    struct total initial = { 0, 0, ~0UL };
    atomic_init(&total, initial);
    if (atomic_is_lock_free(&total))
        return 1;

    pthread_t threads[workers];
    int ids[workers] = { 0, 1 };
    for (int id = 0; id < workers; ++id)
        if (pthread_create(&threads[id], NULL, exercise, &ids[id]))
            return 2;

    int forbidden = 0;
    for (int i = 1; i <= rounds; ++i) {
        atomic_store_explicit(&slots[0], 0, memory_order_relaxed);
        atomic_store_explicit(&slots[1], 0, memory_order_relaxed);
        atomic_store_explicit(&start, i, memory_order_release);
        for (int id = 0; id < workers; ++id)
            while (atomic_load_explicit(&finished[id], memory_order_acquire) != i) {}
        forbidden += observed[0] == 0 && observed[1] == 0;
    }

    for (int id = 0; id < workers; ++id)
        if (pthread_join(threads[id], NULL))
            return 3;
    if (messages != rounds)
        return 4;
    if (forbidden)
        return 5;

    struct total result = atomic_load_explicit(&total, memory_order_acquire);
    if (result.count != workers * rounds || result.sum != 3 * rounds ||
        result.inverse != ~result.count || retries[0] + retries[1] == 0)
        return 6;

    printf("release/acquire %d\n", messages);
    printf("seq_cst %d, forbidden %d\n", rounds, forbidden);
    printf("aggregate CAS %lu, sum %lu\n", result.count, result.sum);
    return 0;
}
