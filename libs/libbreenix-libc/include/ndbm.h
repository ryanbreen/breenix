/* <ndbm.h>: POSIX.1-2024 database functions. libc does not define this header's functions yet
 * (issue #1597), so it declares only the types and constants. */
#ifndef __BREENIX_NDBM_H
#define __BREENIX_NDBM_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#define __need_size_t
#include <stddef.h>
typedef __breenix_mode_t mode_t;

typedef struct {
    void *dptr;
    size_t dsize;
} datum;

typedef struct __breenix_dbm DBM;

#define DBM_INSERT 0
#define DBM_REPLACE 1
#endif /* __BREENIX_POSIX */
#endif
