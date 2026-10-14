/* <strings.h>: POSIX.1-2024, plus the BSD bcopy family under _GNU_SOURCE or
 * _DEFAULT_SOURCE. */
#ifndef __BREENIX_STRINGS_H
#define __BREENIX_STRINGS_H

#define __need_size_t
#define __need_NULL
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

int ffs(int i);
int ffsl(long i);
int ffsll(long long i);
int strcasecmp(const char *a, const char *b);
int strncasecmp(const char *a, const char *b, size_t n);

#if defined(_GNU_SOURCE) || defined(_DEFAULT_SOURCE) || defined(_BSD_SOURCE)
int bcmp(const void *a, const void *b, size_t n);
void bcopy(const void *src, void *dst, size_t n);
void bzero(void *s, size_t n);
char *index(const char *s, int c);
char *rindex(const char *s, int c);
#endif

#ifdef __cplusplus
}
#endif

#endif
