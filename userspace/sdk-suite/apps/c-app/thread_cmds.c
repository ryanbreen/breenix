#define _POSIX_C_SOURCE 200809L

#include <pthread.h>
#include <stdint.h>
#include <stdio.h>

#include "app.h"

#define THREADS 4

static long long results[THREADS];

/* Thread k sums k*1000 .. k*1000+999 and returns a pointer to its result. */
static void *sum_range(void *arg)
{
    intptr_t k = (intptr_t)arg;
    long long sum = 0;
    for (long long i = k * 1000; i < (k + 1) * 1000; i++) {
        sum += i;
    }
    results[k] = sum;
    return &results[k];
}

int cmd_threads(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    pthread_t threads[THREADS];
    for (intptr_t k = 0; k < THREADS; k++) {
        if (pthread_create(&threads[k], NULL, sum_range, (void *)k) != 0) {
            printf("pthread_create failed\n");
            return 1;
        }
    }
    long long total = 0;
    for (int k = 0; k < THREADS; k++) {
        void *result;
        if (pthread_join(threads[k], &result) != 0) {
            printf("pthread_join failed\n");
            return 1;
        }
        total += *(long long *)result;
    }
    printf("threads=%d total=%lld\n", THREADS, total);
    return 0;
}

static pthread_mutex_t counter_lock = PTHREAD_MUTEX_INITIALIZER;
static long counter;

static void *count_up(void *arg)
{
    (void)arg;
    for (int i = 0; i < 10000; i++) {
        pthread_mutex_lock(&counter_lock);
        counter++;
        pthread_mutex_unlock(&counter_lock);
    }
    return NULL;
}

int cmd_mutex(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    pthread_t threads[THREADS];
    for (int k = 0; k < THREADS; k++) {
        if (pthread_create(&threads[k], NULL, count_up, NULL) != 0) {
            printf("pthread_create failed\n");
            return 1;
        }
    }
    for (int k = 0; k < THREADS; k++) {
        pthread_join(threads[k], NULL);
    }
    printf("counter=%ld\n", counter);
    return 0;
}

static pthread_mutex_t slot_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t slot_changed = PTHREAD_COND_INITIALIZER;
static int slot;
static int slot_full;

static void *produce(void *arg)
{
    (void)arg;
    for (int i = 1; i <= 100; i++) {
        pthread_mutex_lock(&slot_lock);
        while (slot_full) {
            pthread_cond_wait(&slot_changed, &slot_lock);
        }
        slot = i;
        slot_full = 1;
        pthread_cond_broadcast(&slot_changed);
        pthread_mutex_unlock(&slot_lock);
    }
    return NULL;
}

int cmd_condvar(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    pthread_t producer;
    if (pthread_create(&producer, NULL, produce, NULL) != 0) {
        printf("pthread_create failed\n");
        return 1;
    }
    int received = 0;
    long sum = 0;
    while (received < 100) {
        pthread_mutex_lock(&slot_lock);
        while (!slot_full) {
            pthread_cond_wait(&slot_changed, &slot_lock);
        }
        sum += slot;
        received++;
        slot_full = 0;
        pthread_cond_broadcast(&slot_changed);
        pthread_mutex_unlock(&slot_lock);
    }
    pthread_join(producer, NULL);
    printf("received=%d sum=%ld\n", received, sum);
    return 0;
}
