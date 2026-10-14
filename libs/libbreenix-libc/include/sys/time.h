/* sys/time.h: time values and interval timers. */
#ifndef _SYS_TIME_H
#define _SYS_TIME_H

#include <__breenix/types.h>
#include <__breenix/timeval.h>
#include <sys/select.h>

typedef __breenix_time_t time_t;
typedef __breenix_suseconds_t suseconds_t;

#define ITIMER_REAL 0
#define ITIMER_VIRTUAL 1
#define ITIMER_PROF 2

struct itimerval {
    struct timeval it_interval;
    struct timeval it_value;
};

__BREENIX_BEGIN_DECLS

int getitimer(int, struct itimerval *);
int setitimer(int, const struct itimerval *__BREENIX_RESTRICT, struct itimerval *__BREENIX_RESTRICT);
int gettimeofday(struct timeval *__BREENIX_RESTRICT, void *__BREENIX_RESTRICT);
int utimes(const char *, const struct timeval[2]);

#ifdef __BREENIX_EXT
struct timezone {
    int tz_minuteswest;
    int tz_dsttime;
};
int settimeofday(const struct timeval *, const struct timezone *);
int futimes(int, const struct timeval[2]);
int lutimes(const char *, const struct timeval[2]);

#define timerisset(t) ((t)->tv_sec || (t)->tv_usec)
#define timerclear(t) ((t)->tv_sec = (t)->tv_usec = 0)
#define timercmp(a, b, op) ((a)->tv_sec == (b)->tv_sec ? \
    (a)->tv_usec op (b)->tv_usec : (a)->tv_sec op (b)->tv_sec)
#define timeradd(a, b, r) do { \
        (r)->tv_sec = (a)->tv_sec + (b)->tv_sec; \
        (r)->tv_usec = (a)->tv_usec + (b)->tv_usec; \
        if ((r)->tv_usec >= 1000000) { (r)->tv_sec++; (r)->tv_usec -= 1000000; } \
    } while (0)
#define timersub(a, b, r) do { \
        (r)->tv_sec = (a)->tv_sec - (b)->tv_sec; \
        (r)->tv_usec = (a)->tv_usec - (b)->tv_usec; \
        if ((r)->tv_usec < 0) { (r)->tv_sec--; (r)->tv_usec += 1000000; } \
    } while (0)
#endif

__BREENIX_END_DECLS

#endif
