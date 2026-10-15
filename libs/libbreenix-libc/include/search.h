/* <search.h>: POSIX.1-2024 search tables and trees. libc does not define this header's functions yet
 * (issue #1599), so it declares only the types and constants. */
#ifndef __BREENIX_SEARCH_H
#define __BREENIX_SEARCH_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#define __need_size_t
#include <stddef.h>

typedef struct entry {
    char *key;
    void *data;
} ENTRY;

typedef enum { FIND, ENTER } ACTION;
typedef enum { preorder, postorder, endorder, leaf } VISIT;
typedef void posix_tnode;
#endif /* __BREENIX_POSIX */
#endif
