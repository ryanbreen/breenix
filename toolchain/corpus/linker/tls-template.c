/* N1570 §§6.2.4, 6.7.5; AMD64 psABI §10.3; Breenix pthread.h. */
#define _POSIX_C_SOURCE 200809L
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>

_Alignas(32) static _Thread_local volatile unsigned initialized = 17;
_Alignas(64) static _Thread_local volatile unsigned zero_fill[4];

struct probe {
    unsigned marker;
    uintptr_t main_address;
    int ok;
};

static int template_ok(void)
{
    return initialized == 17 && zero_fill[0] == 0 && zero_fill[1] == 0 &&
           zero_fill[2] == 0 && zero_fill[3] == 0 &&
           (uintptr_t)&initialized % 32 == 0 && (uintptr_t)zero_fill % 64 == 0;
}

static void *worker(void *arg)
{
    struct probe *probe = arg;
    probe->ok = template_ok() && (uintptr_t)&initialized != probe->main_address;
    initialized = probe->marker;
    zero_fill[3] = probe->marker + 1;
    probe->ok = probe->ok && initialized == probe->marker && zero_fill[3] == probe->marker + 1;
    return NULL;
}

int main(void)
{
    if (!template_ok())
        return 1;
    initialized = 41;
    zero_fill[3] = 42;
    for (unsigned i = 0; i < 2; ++i) {
        pthread_t thread;
        struct probe probe = {60 + i, (uintptr_t)&initialized, 0};
        if (pthread_create(&thread, NULL, worker, &probe) != 0 ||
            pthread_join(thread, NULL) != 0 || !probe.ok ||
            initialized != 41 || zero_fill[3] != 42)
            return 2;
    }
    puts("TLS template and isolation verified in three threads");
    return 0;
}
