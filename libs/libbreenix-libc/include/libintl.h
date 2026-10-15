/* <libintl.h>: POSIX.1-2024 message catalogs. libc does not define this header's functions yet
 * (issue #1595), so it declares only the types and constants. */
#ifndef __BREENIX_LIBINTL_H
#define __BREENIX_LIBINTL_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
typedef __breenix_locale_t locale_t;
#endif /* __BREENIX_POSIX */
#endif
