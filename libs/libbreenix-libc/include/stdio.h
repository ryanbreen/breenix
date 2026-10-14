#ifndef __BREENIX_STDIO_H
#define __BREENIX_STDIO_H
#include <stddef.h>
#include <stdarg.h>
#include <__breenix/features.h>
typedef struct __breenix_file FILE;
typedef struct { long long offset; unsigned char state[8]; } fpos_t;
#define EOF (-1)
#define BUFSIZ 1024
#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define FILENAME_MAX 4096
#define L_tmpnam 64
#define TMP_MAX 238328
extern FILE *stdin, *stdout, *stderr;
#ifdef __cplusplus
extern "C" {
#endif
FILE *fopen(const char *restrict, const char *restrict);
FILE *freopen(const char *restrict, const char *restrict, FILE *restrict);
int fclose(FILE *);
int fflush(FILE *);
int setvbuf(FILE *restrict, char *restrict, int, size_t);
void setbuf(FILE *restrict, char *restrict);
size_t fread(void *restrict, size_t, size_t, FILE *restrict);
size_t fwrite(const void *restrict, size_t, size_t, FILE *restrict);
int fgetc(FILE *); int getc(FILE *); int getchar(void);
int fputc(int, FILE *); int putc(int, FILE *); int putchar(int);
char *fgets(char *restrict, int, FILE *restrict);
int fputs(const char *restrict, FILE *restrict); int puts(const char *);
int ungetc(int, FILE *);
int feof(FILE *); int ferror(FILE *); void clearerr(FILE *);
int fseek(FILE *, long, int); long ftell(FILE *); void rewind(FILE *);
int fgetpos(FILE *restrict, fpos_t *restrict); int fsetpos(FILE *, const fpos_t *);
FILE *tmpfile(void); char *tmpnam(char *);
void perror(const char *);
int remove(const char *); int rename(const char *, const char *);
int printf(const char *restrict, ...);
int fprintf(FILE *restrict, const char *restrict, ...);
int sprintf(char *restrict, const char *restrict, ...);
int snprintf(char *restrict, size_t, const char *restrict, ...);
int vprintf(const char *restrict, va_list);
int vfprintf(FILE *restrict, const char *restrict, va_list);
int vsprintf(char *restrict, const char *restrict, va_list);
int vsnprintf(char *restrict, size_t, const char *restrict, va_list);
int scanf(const char *restrict, ...);
int fscanf(FILE *restrict, const char *restrict, ...);
int sscanf(const char *restrict, const char *restrict, ...);
int vscanf(const char *restrict, va_list);
int vfscanf(FILE *restrict, const char *restrict, va_list);
int vsscanf(const char *restrict, const char *restrict, va_list);
#if !defined(__STRICT_ANSI__) || defined(_POSIX_C_SOURCE) || defined(_XOPEN_SOURCE) || defined(_GNU_SOURCE) || defined(_DEFAULT_SOURCE)
#include <sys/types.h>
FILE *fdopen(int, const char *); int fileno(FILE *);
int fseeko(FILE *, off_t, int); off_t ftello(FILE *);
void flockfile(FILE *); int ftrylockfile(FILE *); void funlockfile(FILE *);
int getc_unlocked(FILE *); int getchar_unlocked(void);
int putc_unlocked(int, FILE *); int putchar_unlocked(int);
ssize_t getline(char **restrict, size_t *restrict, FILE *restrict);
ssize_t getdelim(char **restrict, size_t *restrict, int, FILE *restrict);
FILE *open_memstream(char **, size_t *); FILE *fmemopen(void *, size_t, const char *);
int dprintf(int, const char *restrict, ...); int vdprintf(int, const char *restrict, va_list);
int asprintf(char **restrict, const char *restrict, ...); int vasprintf(char **restrict, const char *restrict, va_list);
#endif
#if defined(_GNU_SOURCE) || defined(_DEFAULT_SOURCE)
int fgetc_unlocked(FILE *); int fputc_unlocked(int, FILE *);
size_t fread_unlocked(void *, size_t, size_t, FILE *);
size_t fwrite_unlocked(const void *, size_t, size_t, FILE *);
int fflush_unlocked(FILE *); int feof_unlocked(FILE *); int ferror_unlocked(FILE *);
void clearerr_unlocked(FILE *);
#endif
#ifdef __cplusplus
}
#endif
#endif
