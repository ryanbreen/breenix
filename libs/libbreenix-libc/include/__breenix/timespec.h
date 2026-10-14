/* struct timespec, shared by time.h, sys/stat.h, sys/select.h and others. */
#ifndef __BREENIX_TIMESPEC_H
#define __BREENIX_TIMESPEC_H
#include <__breenix/types.h>
struct timespec {
    __breenix_time_t tv_sec;
    long tv_nsec;
};
#endif
