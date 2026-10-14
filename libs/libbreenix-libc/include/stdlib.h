/* <stdlib.h>: ISO C17 7.22 and POSIX.1-2024 with XSI, plus the GNU and BSD extensions
 * Breenix implements under _GNU_SOURCE or _DEFAULT_SOURCE. */
#ifndef __BREENIX_STDLIB_H
#define __BREENIX_STDLIB_H

#include <__breenix/features.h>
#include <__breenix/types.h>
#define __need_size_t
#define __need_wchar_t
#define __need_NULL
#include <stddef.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
#define RAND_MAX 2147483647
#define MB_CUR_MAX (__breenix_mb_cur_max())

typedef struct { int quot; int rem; } div_t;
typedef struct { long quot; long rem; } ldiv_t;
typedef struct { long long quot; long long rem; } lldiv_t;

__BREENIX_BEGIN_DECLS

size_t __breenix_mb_cur_max(void);

double atof(const char *s);
int atoi(const char *s);
long atol(const char *s);
long long atoll(const char *s);
double strtod(const char *__BREENIX_RESTRICT s, char **__BREENIX_RESTRICT end);
float strtof(const char *__BREENIX_RESTRICT s, char **__BREENIX_RESTRICT end);
long double strtold(const char *__BREENIX_RESTRICT s, char **__BREENIX_RESTRICT end);
long strtol(const char *__BREENIX_RESTRICT s, char **__BREENIX_RESTRICT end, int base);
long long strtoll(const char *__BREENIX_RESTRICT s, char **__BREENIX_RESTRICT end, int base);
unsigned long strtoul(const char *__BREENIX_RESTRICT s, char **__BREENIX_RESTRICT end, int base);
unsigned long long strtoull(const char *__BREENIX_RESTRICT s, char **__BREENIX_RESTRICT end, int base);

int rand(void);
void srand(unsigned seed);

void *aligned_alloc(size_t alignment, size_t size);
void *calloc(size_t n, size_t size);
void free(void *p);
void *malloc(size_t size);
void *realloc(void *p, size_t size);

__BREENIX_NORETURN void abort(void);
int atexit(void (*f)(void));
int at_quick_exit(void (*f)(void));
__BREENIX_NORETURN void exit(int status);
__BREENIX_NORETURN void _Exit(int status);
__BREENIX_NORETURN void quick_exit(int status);
char *getenv(const char *name);
int system(const char *command);

void *bsearch(const void *key, const void *base, size_t n, size_t size,
              int (*compar)(const void *, const void *));
void qsort(void *base, size_t n, size_t size, int (*compar)(const void *, const void *));

int abs(int x);
long labs(long x);
long long llabs(long long x);
div_t div(int a, int b);
ldiv_t ldiv(long a, long b);
lldiv_t lldiv(long long a, long long b);

int mblen(const char *s, size_t n);
int mbtowc(wchar_t *__BREENIX_RESTRICT pwc, const char *__BREENIX_RESTRICT s, size_t n);
int wctomb(char *s, wchar_t wc);
size_t mbstowcs(wchar_t *__BREENIX_RESTRICT dst, const char *__BREENIX_RESTRICT src, size_t n);
size_t wcstombs(char *__BREENIX_RESTRICT dst, const wchar_t *__BREENIX_RESTRICT src, size_t n);

#ifdef __BREENIX_POSIX
int posix_memalign(void **out, size_t alignment, size_t size);
void *reallocarray(void *p, size_t n, size_t size);
int setenv(const char *name, const char *value, int overwrite);
int unsetenv(const char *name);
int putenv(char *string);
int getsubopt(char **__BREENIX_RESTRICT optionp, char *const *__BREENIX_RESTRICT keylist,
              char **__BREENIX_RESTRICT valuep);
int rand_r(unsigned *seed);
long random(void);
void srandom(unsigned seed);
double drand48(void);
double erand48(unsigned short xsubi[3]);
long lrand48(void);
long nrand48(unsigned short xsubi[3]);
long mrand48(void);
long jrand48(unsigned short xsubi[3]);
void srand48(long seed);
unsigned short *seed48(unsigned short seed16v[3]);
void lcong48(unsigned short param[7]);
void qsort_r(void *base, size_t n, size_t size, int (*compar)(const void *, const void *, void *),
             void *arg);
char *realpath(const char *__BREENIX_RESTRICT path, char *__BREENIX_RESTRICT resolved);
int mkstemp(char *template_);
int mkostemp(char *template_, int flags);
char *mkdtemp(char *template_);
#endif

#ifdef __BREENIX_EXT
int clearenv(void);
void *memalign(size_t alignment, size_t size);
int mkstemps(char *template_, int suffixlen);
int mkostemps(char *template_, int suffixlen, int flags);
#endif

__BREENIX_END_DECLS

#ifdef __BREENIX_EXT
#include <alloca.h>
#endif

#endif
