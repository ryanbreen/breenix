/* <nl_types.h>: POSIX.1-2024 message catalogs. libc does not define this header's functions yet
 * (issue #1598), so it declares only the types and constants. */
#ifndef __BREENIX_NL_TYPES_H
#define __BREENIX_NL_TYPES_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
typedef void *nl_catd;
typedef int nl_item;

#define NL_SETD 1
#define NL_CAT_LOCALE 1
#endif /* __BREENIX_POSIX */
#endif
