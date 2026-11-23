/* N1570 §§6.2.4, 6.7.5; AMD64 psABI §§3.1.2, 10.3; POSIX pthreads. */
#define _POSIX_C_SOURCE 200809L
#include "threads.h"
#include <stdint.h>

static _Thread_local volatile long initialized = 17;
static _Thread_local volatile long zero_fill[2];

struct Probe {
    long marker;
    uintptr_t main_address;
    int ok;
};

static int template_ok(void)
{
    return initialized == 17 && zero_fill[0] == 0 && zero_fill[1] == 0 &&
           (uintptr_t)&initialized % 8 == 0 && (uintptr_t)zero_fill % 8 == 0;
}

static void *worker(void *arg)
{
    struct Probe *probe = arg;
    int fresh = template_ok() && (uintptr_t)&initialized != probe->main_address;
    initialized = probe->marker;
    zero_fill[1] = probe->marker + 1;
    probe->ok = fresh && initialized == probe->marker &&
                zero_fill[0] == 0 && zero_fill[1] == probe->marker + 1;
    return 0;
}

int main(void)
{
    pthread_t threads[2];
    struct Probe probes[2];

    if (!template_ok())
        return 1;
    initialized = 41;
    zero_fill[1] = 42;
    for (int i = 0; i < 2; ++i) {
        probes[i].marker = 60 + i;
        probes[i].main_address = (uintptr_t)&initialized;
        probes[i].ok = 0;
        if (pthread_create(&threads[i], 0, worker, &probes[i]) != 0)
            return 2;
    }
    for (int i = 0; i < 2; ++i)
        if (pthread_join(threads[i], 0) != 0 || !probes[i].ok)
            return 3;

    return initialized != 41 || zero_fill[0] != 0 || zero_fill[1] != 42;
}
