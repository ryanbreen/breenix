#ifndef __BREENIX_WCHAR_H
#define __BREENIX_WCHAR_H
/*
 * C17 7.29.1 names only, so a strictly conforming program may use every other name:
 * size_t, wchar_t and NULL come from the compiler's <stddef.h> pieces, and the
 * prototypes spell FILE and va_list by reserved names. POSIX mode adds the types
 * POSIX.1-2024 <wchar.h> requires (FILE, locale_t, va_list, wctype_t).
 */
#include <__breenix/types.h>
#include <__breenix/features.h>
#define __need_size_t
#define __need_wchar_t
#define __need_NULL
#include <stddef.h>
#define __need___va_list
#include <stdarg.h>
typedef __breenix_wint_t wint_t;
typedef __breenix_mbstate_t mbstate_t;
#define WEOF ((wint_t)-1)
#ifndef WCHAR_MIN
#ifdef __WCHAR_UNSIGNED__
#define WCHAR_MIN 0
#else
#define WCHAR_MIN (-__WCHAR_MAX__ - 1)
#endif
#endif
#ifndef WCHAR_MAX
#define WCHAR_MAX __WCHAR_MAX__
#endif
struct __breenix_file;
struct tm;
int mbsinit(const mbstate_t *);
size_t mbrtowc(wchar_t *, const char *, size_t, mbstate_t *);
size_t mbrlen(const char *, size_t, mbstate_t *);
size_t wcrtomb(char *, wchar_t, mbstate_t *);
size_t mbsrtowcs(wchar_t *, const char **, size_t, mbstate_t *);
size_t wcsrtombs(char *, const wchar_t **, size_t, mbstate_t *);
size_t mbstowcs(wchar_t *, const char *, size_t);
size_t wcstombs(char *, const wchar_t *, size_t);
int mbtowc(wchar_t *, const char *, size_t);
int mblen(const char *, size_t);
int wctomb(char *, wchar_t);
wint_t btowc(int);
int wctob(wint_t);
size_t wcslen(const wchar_t *);
wchar_t *wcscpy(wchar_t *, const wchar_t *);
wchar_t *wcsncpy(wchar_t *, const wchar_t *, size_t);
wchar_t *wcscat(wchar_t *, const wchar_t *);
wchar_t *wcsncat(wchar_t *, const wchar_t *, size_t);
int wcscmp(const wchar_t *, const wchar_t *);
int wcsncmp(const wchar_t *, const wchar_t *, size_t);
int wcscoll(const wchar_t *, const wchar_t *);
size_t wcsxfrm(wchar_t *, const wchar_t *, size_t);
wchar_t *wcschr(const wchar_t *, wchar_t);
wchar_t *wcsrchr(const wchar_t *, wchar_t);
wchar_t *wcsstr(const wchar_t *, const wchar_t *);
size_t wcsspn(const wchar_t *, const wchar_t *);
size_t wcscspn(const wchar_t *, const wchar_t *);
wchar_t *wcspbrk(const wchar_t *, const wchar_t *);
wchar_t *wcstok(wchar_t *, const wchar_t *, wchar_t **);
wchar_t *wmemcpy(wchar_t *, const wchar_t *, size_t);
wchar_t *wmemmove(wchar_t *, const wchar_t *, size_t);
wchar_t *wmemset(wchar_t *, wchar_t, size_t);
wchar_t *wmemchr(const wchar_t *, wchar_t, size_t);
int wmemcmp(const wchar_t *, const wchar_t *, size_t);
long wcstol(const wchar_t *, wchar_t **, int);
unsigned long wcstoul(const wchar_t *, wchar_t **, int);
long long wcstoll(const wchar_t *, wchar_t **, int);
unsigned long long wcstoull(const wchar_t *, wchar_t **, int);
double wcstod(const wchar_t *, wchar_t **);
float wcstof(const wchar_t *, wchar_t **);
long double wcstold(const wchar_t *, wchar_t **);
int fwide(struct __breenix_file *, int);
wint_t fgetwc(struct __breenix_file *);
wint_t getwc(struct __breenix_file *);
wint_t getwchar(void);
wint_t fputwc(wchar_t, struct __breenix_file *);
wint_t putwc(wchar_t, struct __breenix_file *);
wint_t putwchar(wchar_t);
wint_t ungetwc(wint_t, struct __breenix_file *);
wchar_t *fgetws(wchar_t *, int, struct __breenix_file *);
int fputws(const wchar_t *, struct __breenix_file *);
int swprintf(wchar_t *, size_t, const wchar_t *, ...);
int vswprintf(wchar_t *, size_t, const wchar_t *, __gnuc_va_list);
int fwprintf(struct __breenix_file *, const wchar_t *, ...);
int vfwprintf(struct __breenix_file *, const wchar_t *, __gnuc_va_list);
int wprintf(const wchar_t *, ...);
int vwprintf(const wchar_t *, __gnuc_va_list);
int fwscanf(struct __breenix_file *, const wchar_t *, ...);
int swscanf(const wchar_t *, const wchar_t *, ...);
int wscanf(const wchar_t *, ...);
int vfwscanf(struct __breenix_file *, const wchar_t *, __gnuc_va_list);
int vswscanf(const wchar_t *, const wchar_t *, __gnuc_va_list);
int vwscanf(const wchar_t *, __gnuc_va_list);
size_t wcsftime(wchar_t *, size_t, const wchar_t *, const struct tm *);
#if defined(__BREENIX_POSIX)
#define __need_va_list
#include <stdarg.h>
typedef struct __breenix_file FILE;
typedef __breenix_locale_t locale_t;
typedef __breenix_wctype_t wctype_t;
size_t mbsnrtowcs(wchar_t *, const char **, size_t, size_t, mbstate_t *);
size_t wcsnrtombs(char *, const wchar_t **, size_t, size_t, mbstate_t *);
wchar_t *wcpcpy(wchar_t *, const wchar_t *);
wchar_t *wcpncpy(wchar_t *, const wchar_t *, size_t);
size_t wcslcpy(wchar_t *, const wchar_t *, size_t);
size_t wcslcat(wchar_t *, const wchar_t *, size_t);
size_t wcsnlen(const wchar_t *, size_t);
wchar_t *wcsdup(const wchar_t *);
int wcscasecmp(const wchar_t *, const wchar_t *);
int wcsncasecmp(const wchar_t *, const wchar_t *, size_t);
int wcscasecmp_l(const wchar_t *, const wchar_t *, locale_t);
int wcsncasecmp_l(const wchar_t *, const wchar_t *, size_t, locale_t);
int wcscoll_l(const wchar_t *, const wchar_t *, locale_t);
size_t wcsxfrm_l(wchar_t *, const wchar_t *, size_t, locale_t);
int wcwidth(wchar_t);
int wcswidth(const wchar_t *, size_t);
#endif
#endif
