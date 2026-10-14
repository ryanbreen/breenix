/* <strings.h>: POSIX.1-2024, plus the BSD bcopy family under _GNU_SOURCE or
 * _DEFAULT_SOURCE. */
#ifndef __BREENIX_STRINGS_H
#define __BREENIX_STRINGS_H

#include <__breenix/features.h>
#define __need_size_t
#define __need_NULL
#include <stddef.h>
#include <__breenix/types.h>
typedef __breenix_locale_t locale_t;

__BREENIX_BEGIN_DECLS

int ffs(int i);
int ffsl(long i);
int ffsll(long long i);
int strcasecmp(const char *a, const char *b);
int strncasecmp(const char *a, const char *b, size_t n);
int strcasecmp_l(const char *a, const char *b, locale_t loc);
int strncasecmp_l(const char *a, const char *b, size_t n, locale_t loc);

#ifdef __BREENIX_EXT
int bcmp(const void *a, const void *b, size_t n);
void bcopy(const void *src, void *dst, size_t n);
void bzero(void *s, size_t n);
char *index(const char *s, int c);
char *rindex(const char *s, int c);
#endif

__BREENIX_END_DECLS

#endif
