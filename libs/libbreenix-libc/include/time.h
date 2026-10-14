#ifndef __BREENIX_TIME_H
#define __BREENIX_TIME_H
#include <stddef.h>
#include <__breenix/types.h>
#include <__breenix/features.h>
#include <__breenix/timespec.h>
typedef __breenix_time_t time_t;
typedef __breenix_clock_t clock_t;
struct tm {
    int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst;
    long tm_gmtoff;
    const char *tm_zone;
};
#define CLOCKS_PER_SEC 1000000L
#define TIME_UTC 1
time_t time(time_t *);
double difftime(time_t, time_t);
clock_t clock(void);
struct tm *gmtime(const time_t *);
struct tm *localtime(const time_t *);
time_t mktime(struct tm *);
size_t strftime(char *, size_t, const char *, const struct tm *);
char *asctime(const struct tm *);
char *ctime(const time_t *);
int timespec_get(struct timespec *, int);
#if defined(__BREENIX_POSIX)
typedef __breenix_clockid_t clockid_t;
#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
#define CLOCK_PROCESS_CPUTIME_ID 2
#define CLOCK_THREAD_CPUTIME_ID 3
int clock_gettime(clockid_t, struct timespec *);
int clock_getres(clockid_t, struct timespec *);
int clock_settime(clockid_t, const struct timespec *);
int nanosleep(const struct timespec *, struct timespec *);
struct tm *gmtime_r(const time_t *, struct tm *);
struct tm *localtime_r(const time_t *, struct tm *);
char *asctime_r(const struct tm *, char *);
char *ctime_r(const time_t *, char *);
void tzset(void);
extern long timezone;
extern int daylight;
extern char *tzname[2];
time_t timegm(struct tm *);
#endif
#endif
