/*
 * POSIX.1-2024 <fnmatch.h>. Breenix assigns consecutive independent flag
 * bits in declaration order; FNM_NOMATCH is the first nonzero result.
 * CASEFOLD and LEADING_DIR are locally defined optional behaviors.
 */
#ifndef _FNMATCH_H
#define _FNMATCH_H

#include <__breenix/features.h>

__BREENIX_BEGIN_DECLS

#define FNM_NOMATCH 1

#define FNM_PATHNAME 0x01
#define FNM_NOESCAPE 0x02
#define FNM_PERIOD 0x04
#define FNM_CASEFOLD 0x10
#define FNM_IGNORECASE FNM_CASEFOLD

#ifdef __BREENIX_EXT
#define FNM_FILE_NAME FNM_PATHNAME
#define FNM_LEADING_DIR 0x08
#endif

int fnmatch(const char *, const char *, int);

__BREENIX_END_DECLS

#endif
