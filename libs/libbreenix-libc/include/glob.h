/*
 * <glob.h>: pathname pattern expansion (POSIX.1-2024 XBD <glob.h>), with Linux's glob_t
 * layout and flag values. The five pointers after gl_flags keep Linux's size; they belong
 * to GNU's GLOB_ALTDIRFUNC, which Breenix does not offer.
 */
#ifndef _GLOB_H
#define _GLOB_H

#include <__breenix/features.h>
#include <__breenix/types.h>

__BREENIX_BEGIN_DECLS

typedef __breenix_size_t size_t;

typedef struct {
    size_t gl_pathc;
    char **gl_pathv;
    size_t gl_offs;
    int gl_flags;
    void *__gl_reserved[5];
} glob_t;

#define GLOB_ERR 0x01
#define GLOB_MARK 0x02
#define GLOB_NOSORT 0x04
#define GLOB_DOOFFS 0x08
#define GLOB_NOCHECK 0x10
#define GLOB_APPEND 0x20
#define GLOB_NOESCAPE 0x40

#ifdef __BREENIX_EXT
#define GLOB_PERIOD 0x80
#define GLOB_MAGCHAR 0x100
#define GLOB_NOMAGIC 0x800
#endif

#define GLOB_NOSPACE 1
#define GLOB_ABORTED 2
#define GLOB_NOMATCH 3

int glob(const char *__BREENIX_RESTRICT, int, int (*)(const char *, int), glob_t *__BREENIX_RESTRICT);
void globfree(glob_t *);

__BREENIX_END_DECLS

#endif
