/* N1570 §7.5: errno has thread storage duration. POSIX pthreads. */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include "threads.h"

struct Probe {
    int marker;
    int *main_errno;
    int ok;
};

static void *worker(void *arg)
{
    struct Probe *probe = arg;
    int *local = &errno;
    errno = probe->marker;
    probe->ok = local != probe->main_errno && *local == probe->marker;
    return 0;
}

int main(void)
{
    pthread_t threads[2];
    struct Probe probes[2];
    int *main_errno = &errno;
    errno = EINVAL;
    for (int i = 0; i < 2; ++i) {
        probes[i].marker = i == 0 ? EIO : ERANGE;
        probes[i].main_errno = main_errno;
        probes[i].ok = 0;
        if (pthread_create(&threads[i], 0, worker, &probes[i]) != 0)
            return 1;
    }
    for (int i = 0; i < 2; ++i)
        if (pthread_join(threads[i], 0) != 0 || !probes[i].ok)
            return 2;

    return errno != EINVAL;
}
