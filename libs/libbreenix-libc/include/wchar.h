#ifndef __BREENIX_WCHAR_H
#define __BREENIX_WCHAR_H
#include <stddef.h>
#include <stdarg.h>
#include <stdint.h>
#include <locale.h>
#include <stdio.h>
typedef __breenix_wint_t wint_t;
typedef __breenix_mbstate_t mbstate_t;
#define WEOF ((wint_t)-1)
#ifndef WCHAR_MIN
#define WCHAR_MIN 0
#endif
#ifndef WCHAR_MAX
#define WCHAR_MAX __WCHAR_MAX__
#endif
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
int fwide(FILE *, int);
wint_t fgetwc(FILE *);
wint_t getwc(FILE *);
wint_t getwchar(void);
wint_t fputwc(wchar_t, FILE *);
wint_t putwc(wchar_t, FILE *);
wint_t putwchar(wchar_t);
wint_t ungetwc(wint_t, FILE *);
wchar_t *fgetws(wchar_t *, int, FILE *);
int fputws(const wchar_t *, FILE *);
int swprintf(wchar_t *, size_t, const wchar_t *, ...);
int vswprintf(wchar_t *, size_t, const wchar_t *, va_list);
int fwprintf(FILE *, const wchar_t *, ...);
int vfwprintf(FILE *, const wchar_t *, va_list);
int wprintf(const wchar_t *, ...);
int vwprintf(const wchar_t *, va_list);
struct tm;
size_t wcsftime(wchar_t *, size_t, const wchar_t *, const struct tm *);
#if defined(__BREENIX_POSIX)
size_t wcsnlen(const wchar_t *, size_t);
wchar_t *wcsdup(const wchar_t *);
int wcscasecmp(const wchar_t *, const wchar_t *);
int wcsncasecmp(const wchar_t *, const wchar_t *, size_t);
#endif
#endif
