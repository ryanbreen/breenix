/* sys/times.h: process times. */
#ifndef _SYS_TIMES_H
#define _SYS_TIMES_H

#include <__breenix/types.h>

typedef __breenix_clock_t clock_t;

struct tms {
    clock_t tms_utime;
    clock_t tms_stime;
    clock_t tms_cutime;
    clock_t tms_cstime;
};

__BREENIX_BEGIN_DECLS

clock_t times(struct tms *);

__BREENIX_END_DECLS

#endif
