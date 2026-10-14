/*
 * POSIX.1-2024 <regex.h>. Breenix assigns consecutive independent flag
 * bits per flag namespace and positive error codes in declaration order.
 * REG_STARTEND is our bounded-region extension.
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
} regex_t;

typedef struct {
    regoff_t rm_so;
    regoff_t rm_eo;
} regmatch_t;

#define REG_EXTENDED 1
#define REG_ICASE 2
#define REG_NEWLINE 4
#define REG_NOSUB 8
/* Next independent bit assigned by this implementation, per POSIX.1-2024. */
#define REG_MINIMAL 16

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
