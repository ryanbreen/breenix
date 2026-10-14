/* utime.h: access and modification times. */
#ifndef _UTIME_H
#define _UTIME_H

#include <__breenix/types.h>

typedef __breenix_time_t time_t;

struct utimbuf {
    time_t actime;
    time_t modtime;
};

__BREENIX_BEGIN_DECLS

int utime(const char *, const struct utimbuf *);

__BREENIX_END_DECLS

#endif
