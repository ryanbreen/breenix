#ifndef __BREENIX_STDIO_H
#define __BREENIX_STDIO_H
#include <stddef.h>
#include <stdarg.h>
#include <__breenix/features.h>
typedef struct __breenix_file FILE;
typedef struct { long long offset; unsigned char state[8]; } fpos_t;
#define EOF (-1)
#define BUFSIZ 1024
#define FOPEN_MAX 16
#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define FILENAME_MAX 4096
#define L_tmpnam 64
#define TMP_MAX 238328
#ifdef __BREENIX_POSIX
#include <sys/types.h>
#endif
__BREENIX_BEGIN_DECLS
extern FILE *stdin, *stdout, *stderr;
FILE *fopen(const char *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT);
FILE *freopen(const char *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, FILE *__BREENIX_RESTRICT);
int fclose(FILE *);
int fflush(FILE *);
int setvbuf(FILE *__BREENIX_RESTRICT, char *__BREENIX_RESTRICT, int, size_t);
void setbuf(FILE *__BREENIX_RESTRICT, char *__BREENIX_RESTRICT);
size_t fread(void *__BREENIX_RESTRICT, size_t, size_t, FILE *__BREENIX_RESTRICT);
size_t fwrite(const void *__BREENIX_RESTRICT, size_t, size_t, FILE *__BREENIX_RESTRICT);
int fgetc(FILE *); int getc(FILE *); int getchar(void);
int fputc(int, FILE *); int putc(int, FILE *); int putchar(int);
char *fgets(char *__BREENIX_RESTRICT, int, FILE *__BREENIX_RESTRICT);
int fputs(const char *__BREENIX_RESTRICT, FILE *__BREENIX_RESTRICT); int puts(const char *);
int ungetc(int, FILE *);
int feof(FILE *); int ferror(FILE *); void clearerr(FILE *);
int fseek(FILE *, long, int); long ftell(FILE *); void rewind(FILE *);
int fgetpos(FILE *__BREENIX_RESTRICT, fpos_t *__BREENIX_RESTRICT); int fsetpos(FILE *, const fpos_t *);
FILE *tmpfile(void); char *tmpnam(char *);
void perror(const char *);
int remove(const char *); int rename(const char *, const char *);
int printf(const char *__BREENIX_RESTRICT, ...);
int fprintf(FILE *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, ...);
int sprintf(char *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, ...);
int snprintf(char *__BREENIX_RESTRICT, size_t, const char *__BREENIX_RESTRICT, ...);
int vprintf(const char *__BREENIX_RESTRICT, va_list);
int vfprintf(FILE *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, va_list);
int vsprintf(char *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, va_list);
int vsnprintf(char *__BREENIX_RESTRICT, size_t, const char *__BREENIX_RESTRICT, va_list);
int scanf(const char *__BREENIX_RESTRICT, ...);
int fscanf(FILE *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, ...);
int sscanf(const char *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, ...);
int vscanf(const char *__BREENIX_RESTRICT, va_list);
int vfscanf(FILE *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, va_list);
int vsscanf(const char *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, va_list);
#ifdef __BREENIX_POSIX
FILE *fdopen(int, const char *); int fileno(FILE *);
int fseeko(FILE *, off_t, int); off_t ftello(FILE *);
void flockfile(FILE *); int ftrylockfile(FILE *); void funlockfile(FILE *);
int getc_unlocked(FILE *); int getchar_unlocked(void);
int putc_unlocked(int, FILE *); int putchar_unlocked(int);
ssize_t getline(char **__BREENIX_RESTRICT, size_t *__BREENIX_RESTRICT, FILE *__BREENIX_RESTRICT);
ssize_t getdelim(char **__BREENIX_RESTRICT, size_t *__BREENIX_RESTRICT, int, FILE *__BREENIX_RESTRICT);
FILE *open_memstream(char **, size_t *); FILE *fmemopen(void *, size_t, const char *);
int dprintf(int, const char *__BREENIX_RESTRICT, ...); int vdprintf(int, const char *__BREENIX_RESTRICT, va_list);
int asprintf(char **__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, ...); int vasprintf(char **__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, va_list);
#endif
#ifdef __BREENIX_EXT
int fgetc_unlocked(FILE *); int fputc_unlocked(int, FILE *);
size_t fread_unlocked(void *, size_t, size_t, FILE *);
size_t fwrite_unlocked(const void *, size_t, size_t, FILE *);
int fflush_unlocked(FILE *); int feof_unlocked(FILE *); int ferror_unlocked(FILE *);
void clearerr_unlocked(FILE *);
#endif
__BREENIX_END_DECLS
#endif
