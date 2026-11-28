#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/auxv.h>
#include <unistd.h>

static int observed;
static int observed_missing;
static unsigned long observed_cap2;
static volatile size_t copy_size = 23;
static const char source[] = "startup dispatch check";
static char destination[sizeof source];

void check_startup(void)
{
    errno = 0;
    observed_cap2 = getauxval(AT_HWCAP2);
    observed_missing = errno;
    observed = MINSIGSTKSZ;
    void *(*volatile copy)(void *, const void *, size_t) = memcpy;
    copy(destination, source, copy_size);
}

#ifndef DYNAMIC
__attribute__((section(".preinit_array"), used))
static void (*const preinit)(void) = check_startup;
#endif

int main(int argc, char **argv)
{
    if (argc != 2)
        return 1;
    int absent = argv[1][0] == 'a';
    int enabled = argv[1][0] == 'e';
#ifdef __x86_64__
    const int floor = 2048;
    const int default_size = 8192;
    const int cap2 = enabled ? HWCAP2_ERMS : 0;
#else
    const int floor = 5120;
    const int default_size = 16384;
    const int cap2 = 0;
#endif
    if (observed_missing != (absent ? ENOENT : 0) || observed_cap2 != (unsigned long)cap2)
        return 2;
    const int minimum = enabled ? 20000 : floor;
    if (observed != minimum || MINSIGSTKSZ != (size_t)minimum ||
        SIGSTKSZ != (size_t)(minimum + default_size - floor))
        return 3;
    if (memcmp(destination, source, sizeof source) != 0)
        return 4;
    size_t thread_min = minimum + 16384 - floor;
    pthread_attr_t attr;
    if (PTHREAD_STACK_MIN != thread_min || sysconf(_SC_THREAD_STACK_MIN) != (long)thread_min ||
        pthread_attr_init(&attr) != 0 ||
        pthread_attr_setstacksize(&attr, thread_min - 1) != EINVAL ||
        pthread_attr_setstacksize(&attr, thread_min) != 0)
        return 5;
    char formatted[16];
    if (snprintf(formatted, sizeof formatted, "%.2f|%.1Lf", 1.25, 2.5L) != 8 ||
        strcmp(formatted, "1.25|2.5") != 0)
        return 6;
#ifdef DYNAMIC
    void *late = dlopen("/lib/late.so", RTLD_NOW | RTLD_LOCAL);
    if (!late)
        return 7;
    void (*check_late)(void) = (void (*)(void))dlsym(late, "check_late");
    if (!check_late)
        return 8;
    observed = 0;
    check_late();
    if (observed != minimum || observed_missing != (absent ? ENOENT : 0))
        return 9;
#endif
    return 0;
}
