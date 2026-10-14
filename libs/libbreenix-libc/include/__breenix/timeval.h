/* struct timeval, shared by sys/time.h, sys/select.h and sys/resource.h. */
#ifndef __BREENIX_TIMEVAL_H
#define __BREENIX_TIMEVAL_H
#include <__breenix/types.h>
struct timeval {
    __breenix_time_t tv_sec;
    __breenix_suseconds_t tv_usec;
};
#endif
