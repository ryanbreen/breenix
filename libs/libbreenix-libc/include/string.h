/* <string.h>: ISO C17 7.24 and POSIX.1-2024, with the GNU and BSD extensions Breenix
 * implements under _GNU_SOURCE or _DEFAULT_SOURCE. */
#ifndef __BREENIX_STRING_H
#define __BREENIX_STRING_H

#include <__breenix/features.h>
#define __need_size_t
#define __need_NULL
#include <stddef.h>
#ifdef __BREENIX_POSIX
#include <__breenix/types.h>
typedef __breenix_locale_t locale_t;
#endif

__BREENIX_BEGIN_DECLS

void *memcpy(void *__BREENIX_RESTRICT dst, const void *__BREENIX_RESTRICT src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *s, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
void *memchr(const void *s, int c, size_t n);

char *strcpy(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src);
char *strncpy(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t n);
char *strcat(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src);
char *strncat(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t n);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
int strcoll(const char *a, const char *b);
size_t strxfrm(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t n);
char *strchr(const char *s, int c);
char *strrchr(const char *s, int c);
size_t strspn(const char *s, const char *accept);
size_t strcspn(const char *s, const char *reject);
char *strpbrk(const char *s, const char *accept);
char *strstr(const char *haystack, const char *needle);
char *strtok(char *__BREENIX_RESTRICT s, const char *__BREENIX_RESTRICT delim);
size_t strlen(const char *s);
char *strerror(int errnum);

#ifdef __BREENIX_POSIX
void *memccpy(void *__BREENIX_RESTRICT dst, const void *__BREENIX_RESTRICT src, int c, size_t n);
void *memmem(const void *haystack, size_t hn, const void *needle, size_t nn);
char *stpcpy(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src);
char *stpncpy(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t n);
char *strdup(const char *s);
char *strndup(const char *s, size_t n);
size_t strnlen(const char *s, size_t max);
char *strtok_r(char *__BREENIX_RESTRICT s, const char *__BREENIX_RESTRICT delim, char **__BREENIX_RESTRICT save);
int strerror_r(int errnum, char *buf, size_t len);
char *strsignal(int sig);
size_t strlcpy(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t size);
size_t strlcat(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t size);
int strcoll_l(const char *a, const char *b, locale_t loc);
size_t strxfrm_l(char *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t n, locale_t loc);
char *strerror_l(int errnum, locale_t loc);
#endif

#ifdef __BREENIX_EXT
void *mempcpy(void *__BREENIX_RESTRICT dst, const void *__BREENIX_RESTRICT src, size_t n);
void *memrchr(const void *s, int c, size_t n);
char *strchrnul(const char *s, int c);
char *strsep(char **__BREENIX_RESTRICT sp, const char *__BREENIX_RESTRICT delim);
char *strcasestr(const char *haystack, const char *needle);
void explicit_bzero(void *s, size_t n);
#endif

__BREENIX_END_DECLS

#ifdef __BREENIX_EXT
#include <strings.h>
#endif

#endif
