/*
 * <regex.h>: regular expressions (POSIX.1-2024 XBD <regex.h>), with Linux's flag and
 * error values. regoff_t is long, wide enough for any ssize_t as POSIX.1-2024 requires.
 * REG_STARTEND is a GNU and BSD extension.
 */
#ifndef _REGEX_H
#define _REGEX_H

#include <__breenix/features.h>
#include <__breenix/types.h>

__BREENIX_BEGIN_DECLS

typedef __breenix_size_t size_t;
typedef long regoff_t;

typedef struct {
    size_t re_nsub;
    void *__re_prog;
    int __re_cflags;
    int __re_pad;
    void *__re_reserved[5];
} regex_t;

typedef struct {
    regoff_t rm_so;
    regoff_t rm_eo;
} regmatch_t;

#define REG_EXTENDED 1
#define REG_ICASE 2
#define REG_NEWLINE 4
#define REG_NOSUB 8

#define REG_NOTBOL 1
#define REG_NOTEOL 2
#ifdef __BREENIX_EXT
#define REG_STARTEND 4
#endif

#define REG_NOMATCH 1
#define REG_BADPAT 2
#define REG_ECOLLATE 3
#define REG_ECTYPE 4
#define REG_EESCAPE 5
#define REG_ESUBREG 6
#define REG_EBRACK 7
#define REG_EPAREN 8
#define REG_EBRACE 9
#define REG_BADBR 10
#define REG_ERANGE 11
#define REG_ESPACE 12
#define REG_BADRPT 13

int regcomp(regex_t *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, int);
int regexec(const regex_t *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, size_t,
            regmatch_t *__BREENIX_RESTRICT, int);
size_t regerror(int, const regex_t *__BREENIX_RESTRICT, char *__BREENIX_RESTRICT, size_t);
void regfree(regex_t *);

__BREENIX_END_DECLS

#endif
