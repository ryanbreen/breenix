/* <string.h>: ISO C17 7.24 and POSIX.1-2024, with the GNU and BSD extensions Breenix
 * implements under _GNU_SOURCE or _DEFAULT_SOURCE. */
#ifndef __BREENIX_STRING_H
#define __BREENIX_STRING_H

#define __need_size_t
#define __need_NULL
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void *memcpy(void *__restrict dst, const void *__restrict src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *s, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
void *memchr(const void *s, int c, size_t n);

char *strcpy(char *__restrict dst, const char *__restrict src);
char *strncpy(char *__restrict dst, const char *__restrict src, size_t n);
char *strcat(char *__restrict dst, const char *__restrict src);
char *strncat(char *__restrict dst, const char *__restrict src, size_t n);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
int strcoll(const char *a, const char *b);
size_t strxfrm(char *__restrict dst, const char *__restrict src, size_t n);
char *strchr(const char *s, int c);
char *strrchr(const char *s, int c);
size_t strspn(const char *s, const char *accept);
size_t strcspn(const char *s, const char *reject);
char *strpbrk(const char *s, const char *accept);
char *strstr(const char *haystack, const char *needle);
char *strtok(char *__restrict s, const char *__restrict delim);
size_t strlen(const char *s);
char *strerror(int errnum);

#if !defined(__STRICT_ANSI__) || defined(_POSIX_C_SOURCE) || defined(_XOPEN_SOURCE) || \
    defined(_GNU_SOURCE) || defined(_DEFAULT_SOURCE) || defined(_BSD_SOURCE)
void *memccpy(void *__restrict dst, const void *__restrict src, int c, size_t n);
void *memmem(const void *haystack, size_t hn, const void *needle, size_t nn);
char *stpcpy(char *__restrict dst, const char *__restrict src);
char *stpncpy(char *__restrict dst, const char *__restrict src, size_t n);
char *strdup(const char *s);
char *strndup(const char *s, size_t n);
size_t strnlen(const char *s, size_t max);
char *strtok_r(char *__restrict s, const char *__restrict delim, char **__restrict save);
int strerror_r(int errnum, char *buf, size_t len);
char *strsignal(int sig);
size_t strlcpy(char *__restrict dst, const char *__restrict src, size_t size);
size_t strlcat(char *__restrict dst, const char *__restrict src, size_t size);
#endif

#if defined(_GNU_SOURCE) || defined(_DEFAULT_SOURCE) || defined(_BSD_SOURCE)
void *mempcpy(void *__restrict dst, const void *__restrict src, size_t n);
void *memrchr(const void *s, int c, size_t n);
char *strchrnul(const char *s, int c);
char *strsep(char **__restrict sp, const char *__restrict delim);
char *strcasestr(const char *haystack, const char *needle);
void explicit_bzero(void *s, size_t n);
#include <strings.h>
#endif

#ifdef __cplusplus
}
#endif

#endif
