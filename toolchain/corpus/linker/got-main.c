/* AMD64 psABI §§3.2, 4.4: call across archive members and load via the GOT. */
#define _POSIX_C_SOURCE 200809L
#include <pthread.h>
#include <stdio.h>

int archive_entry(void);

static void *worker(void *arg)
{
    int *result = arg;
    *result = archive_entry();
    return NULL;
}

int main(void)
{
    if (archive_entry() != 42)
        return 1;
    puts("archive and GOT agree");
    for (unsigned i = 0; i < 2; ++i) {
        pthread_t thread;
        int result = 0;
        if (pthread_create(&thread, NULL, worker, &result) != 0 ||
            pthread_join(thread, NULL) != 0 || result != 42)
            return 2;
    }
    puts("archive and GOT agree in two new threads");
    return 0;
}
