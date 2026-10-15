/* <monetary.h>: POSIX.1-2024 monetary formatting. libc does not define this header's functions yet
 * (issue #1596), so it declares only the types and constants. */
#ifndef __BREENIX_MONETARY_H
#define __BREENIX_MONETARY_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#define __need_size_t
#include <stddef.h>
typedef __breenix_ssize_t ssize_t;
typedef __breenix_locale_t locale_t;
#endif /* __BREENIX_POSIX */
#endif
